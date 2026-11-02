use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};
use pyth_sdk_solana::load_price_feed_from_account_info;

declare_id!("Aeon1111111111111111111111111111111111111111");

pub const MAX_ORACLE_STALENESS_SLOTS: u64 = 5;
pub const MAX_CONFIDENCE_RATIO_BPS: u64 = 10; // 0.1% max confidence interval
pub const MAINTENANCE_MARGIN_BPS: u64 = 500;  // 5.00%
pub const LIQUIDATION_FEE_BPS: u64 = 200;      // 2.00%
pub const KEEPER_REWARD_RATIO_BPS: u64 = 2000; // 20% of liquidation fee to keeper

#[program]
pub mod aeonian_trade {
    use super::*;

    pub fn initialize_pool(
        ctx: Context<InitializePool>,
        pool_id: u64,
        max_leverage: u8,
    ) -> Result<()> {
        let pool = &mut ctx.accounts.pool_vault;
        pool.authority = ctx.accounts.authority.key();
        pool.collateral_mint = ctx.accounts.collateral_mint.key();
        pool.pool_id = pool_id;
        pool.total_liquidity = 0;
        pool.total_long_open_interest = 0;
        pool.total_short_open_interest = 0;
        pool.max_leverage = max_leverage;
        pool.bump = ctx.bumps.pool_vault;
        Ok(())
    }

    pub fn open_position(
        ctx: Context<OpenPosition>,
        size: u64,
        collateral_amount: u64,
        is_long: bool,
    ) -> Result<()> {
        let clock = Clock::get()?;
        let pyth_feed = load_price_feed_from_account_info(&ctx.accounts.oracle_account)
            .map_err(|_| ErrorCode::InvalidOracleFeed)?;
        
        let current_price_data = pyth_feed.get_price_no_older_than(
            clock.unix_timestamp,
            MAX_ORACLE_STALENESS_SLOTS * 400 / 1000 + 2, // Timestamp tolerance
        ).ok_or(ErrorCode::StaleOraclePrice)?;

        require!(current_price_data.price > 0, ErrorCode::NegativePrice);
        let oracle_price = current_price_data.price as u64;
        let conf = current_price_data.conf;

        // Confidence validation: conf / price <= 0.1%
        let conf_ratio = (conf as u128)
            .checked_mul(10_000)
            .unwrap()
            .checked_div(oracle_price as u128)
            .unwrap();
        require!(conf_ratio <= MAX_CONFIDENCE_RATIO_BPS as u128, ErrorCode::ExcessiveOracleConfidenceInterval);

        // Leverage validation
        let max_lev = ctx.accounts.pool_vault.max_leverage as u64;
        let position_notional = (size as u128)
            .checked_mul(oracle_price as u128)
            .unwrap()
            .checked_div(1_000_000) // Standard decimals normalize
            .unwrap() as u64;

        let required_min_collateral = position_notional
            .checked_div(max_lev)
            .ok_or(ErrorCode::MathOverflow)?;
        require!(collateral_amount >= required_min_collateral, ErrorCode::LeverageExceedsMax);

        // CPI: Transfer collateral into protocol vault
        let cpi_accounts = Transfer {
            from: ctx.accounts.user_token_account.to_account_info(),
            to: ctx.accounts.vault_token_account.to_account_info(),
            authority: ctx.accounts.user.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        token::transfer(CpiContext::new(cpi_program, cpi_accounts), collateral_amount)?;

        // Update Zero-Copy Position State
        let mut position = ctx.accounts.position.load_init()?;
        position.owner = ctx.accounts.user.key();
        position.pool = ctx.accounts.pool_vault.key();
        position.size = size;
        position.collateral = collateral_amount;
        position.entry_price = oracle_price;
        position.last_updated_slot = clock.slot;
        position.is_long = if is_long { 1 } else { 0 };

        // Update Pool Metrics
        let pool = &mut ctx.accounts.pool_vault;
        pool.total_liquidity = pool.total_liquidity.checked_add(collateral_amount).unwrap();
        if is_long {
            pool.total_long_open_interest = pool.total_long_open_interest.checked_add(size).unwrap();
        } else {
            pool.total_short_open_interest = pool.total_short_open_interest.checked_add(size).unwrap();
        }

        emit!(PositionOpenedEvent {
            user: ctx.accounts.user.key(),
            size,
            collateral: collateral_amount,
            entry_price: oracle_price,
            is_long,
            slot: clock.slot,
        });

        Ok(())
    }

    pub fn liquidate_position(ctx: Context<LiquidatePosition>) -> Result<()> {
        let clock = Clock::get()?;
        let pyth_feed = load_price_feed_from_account_info(&ctx.accounts.oracle_account)
            .map_err(|_| ErrorCode::InvalidOracleFeed)?;
        
        let current_price_data = pyth_feed.get_price_no_older_than(
            clock.unix_timestamp,
            MAX_ORACLE_STALENESS_SLOTS * 400 / 1000 + 2,
        ).ok_or(ErrorCode::StaleOraclePrice)?;

        let current_price = current_price_data.price as u64;

        let mut position = ctx.accounts.position.load_mut()?;
        require!(position.size > 0, ErrorCode::PositionInactive);

        let is_long = position.is_long == 1;
        let entry_price = position.entry_price;
        let size = position.size;
        let collateral = position.collateral;

        // Calculate Unrealized PnL
        let notional_entry = (size as u128).checked_mul(entry_price as u128).unwrap();
        let notional_current = (size as u128).checked_mul(current_price as u128).unwrap();

        let (has_profit, pnl_delta) = if is_long {
            if notional_current >= notional_entry {
                (true, (notional_current - notional_entry) / 1_000_000)
            } else {
                (false, (notional_entry - notional_current) / 1_000_000)
            }
        } else {
            if notional_entry >= notional_current {
                (true, (notional_entry - notional_current) / 1_000_000)
            } else {
                (false, (notional_current - notional_entry) / 1_000_000)
            }
        };

        // Compute effective remaining margin
        let remaining_margin: i128 = if has_profit {
            (collateral as i128).checked_add(pnl_delta as i128).unwrap()
        } else {
            (collateral as i128).checked_sub(pnl_delta as i128).unwrap()
        };

        let notional_current_norm = (notional_current / 1_000_000) as u64;
        let maintenance_threshold = (notional_current_norm as u128)
            .checked_mul(MAINTENANCE_MARGIN_BPS as u128)
            .unwrap()
            .checked_div(10_000)
            .unwrap() as i128;

        // Liquidation Condition: Remaining Margin <= Maintenance Threshold
        require!(remaining_margin <= maintenance_threshold, ErrorCode::PositionHealthy);

        // Liquidation Penalty Calculation
        let liquidation_fee = (notional_current_norm as u128)
            .checked_mul(LIQUIDATION_FEE_BPS as u128)
            .unwrap()
            .checked_div(10_000)
            .unwrap() as u64;

        let keeper_reward = (liquidation_fee as u128)
            .checked_mul(KEEPER_REWARD_RATIO_BPS as u128)
            .unwrap()
            .checked_div(10_000)
            .unwrap() as u64;

        let insurance_cut = liquidation_fee.saturating_sub(keeper_reward);

        // Signer seeds for Vault PDA transfer
        let pool_id_bytes = ctx.accounts.pool_vault.pool_id.to_le_bytes();
        let seeds = &[
            b"pool_vault".as_ref(),
            pool_id_bytes.as_ref(),
            &[ctx.accounts.pool_vault.bump],
        ];
        let signer = &[&seeds[..]];

        // Transfer reward to keeper
        let cpi_keeper = Transfer {
            from: ctx.accounts.vault_token_account.to_account_info(),
            to: ctx.accounts.keeper_token_account.to_account_info(),
            authority: ctx.accounts.pool_vault.to_account_info(),
        };
        token::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                cpi_keeper,
                signer,
            ),
            keeper_reward,
        )?;

        // Transfer residual fee to Insurance Fund
        let cpi_insurance = Transfer {
            from: ctx.accounts.vault_token_account.to_account_info(),
            to: ctx.accounts.insurance_vault_token_account.to_account_info(),
            authority: ctx.accounts.pool_vault.to_account_info(),
        };
        token::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                cpi_insurance,
                signer,
            ),
            insurance_cut,
        )?;

        // Update Pool State
        let pool = &mut ctx.accounts.pool_vault;
        if is_long {
            pool.total_long_open_interest = pool.total_long_open_interest.saturating_sub(size);
        } else {
            pool.total_short_open_interest = pool.total_short_open_interest.saturating_sub(size);
        }

        // Reset Position Data
        position.size = 0;
        position.collateral = 0;
        position.entry_price = 0;

        emit!(LiquidatePositionEvent {
            liquidator: ctx.accounts.keeper.key(),
            owner: position.owner,
            position: ctx.accounts.position.key(),
            keeper_reward,
            insurance_cut,
            slot: clock.slot,
        });

        Ok(())
    }
}

// -----------------------------
// ACCOUNT VALIDATION CONTEXTS
// -----------------------------

#[derive(Accounts)]
#[instruction(pool_id: u64)]
pub struct InitializePool<'info> {
    #[account(
        init,
        payer = authority,
        space = 8 + PoolVault::LEN,
        seeds = [b"pool_vault", pool_id.to_le_bytes().as_ref()],
        bump
    )]
    pub pool_vault: Account<'info, PoolVault>,
    pub collateral_mint: Account<'info, Mint>,
    #[account(mut)]
    pub authority: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct OpenPosition<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(
        mut,
        seeds = [b"pool_vault", pool_vault.pool_id.to_le_bytes().as_ref()],
        bump = pool_vault.bump
    )]
    pub pool_vault: Account<'info, PoolVault>,

    #[account(
        init,
        payer = user,
        space = 8 + std::mem::size_of::<PositionState>(),
        seeds = [b"position", user.key().as_ref(), pool_vault.key().as_ref()],
        bump
    )]
    pub position: AccountLoader<'info, PositionState>,

    #[account(
        mut,
        constraint = user_token_account.owner == user.key(),
        constraint = user_token_account.mint == pool_vault.collateral_mint
    )]
    pub user_token_account: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = vault_token_account.owner == pool_vault.key(),
        constraint = vault_token_account.mint == pool_vault.collateral_mint
    )]
    pub vault_token_account: Account<'info, TokenAccount>,

    /// CHECK: Validated inside instruction via Pyth SDK deserializer
    pub oracle_account: AccountInfo<'info>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct LiquidatePosition<'info> {
    #[account(mut)]
    pub keeper: Signer<'info>,

    #[account(
        mut,
        seeds = [b"pool_vault", pool_vault.pool_id.to_le_bytes().as_ref()],
        bump = pool_vault.bump
    )]
    pub pool_vault: Account<'info, PoolVault>,

    #[account(
        mut,
        seeds = [b"position", position.load()?.owner.as_ref(), pool_vault.key().as_ref()],
        bump
    )]
    pub position: AccountLoader<'info, PositionState>,

    #[account(
        mut,
        constraint = vault_token_account.owner == pool_vault.key()
    )]
    pub vault_token_account: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = keeper_token_account.mint == pool_vault.collateral_mint
    )]
    pub keeper_token_account: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = insurance_vault_token_account.mint == pool_vault.collateral_mint
    )]
    pub insurance_vault_token_account: Account<'info, TokenAccount>,

    /// CHECK: Validated via Pyth SDK inside instruction
    pub oracle_account: AccountInfo<'info>,

    pub token_program: Program<'info, Token>,
}

// -----------------------------
// DATA STRUCTURES & EVENTS
// -----------------------------

#[account]
pub struct PoolVault {
    pub authority: Pubkey,
    pub collateral_mint: Pubkey,
    pub pool_id: u64,
    pub total_liquidity: u64,
    pub total_long_open_interest: u64,
    pub total_short_open_interest: u64,
    pub max_leverage: u8,
    pub bump: u8,
}

impl PoolVault {
    pub const LEN: usize = 32 + 32 + 8 + 8 + 8 + 8 + 1 + 1;
}

#[account(zero_copy)]
#[repr(C)]
pub struct PositionState {
    pub owner: Pubkey,         // 32
    pub pool: Pubkey,          // 32
    pub size: u64,              // 8
    pub collateral: u64,        // 8
    pub entry_price: u64,       // 8
    pub last_updated_slot: u64, // 8
    pub is_long: u8,            // 1
    pub _padding: [u8; 7],      // 7
}

#[event]
pub struct PositionOpenedEvent {
    pub user: Pubkey,
    pub size: u64,
    pub collateral: u64,
    pub entry_price: u64,
    pub is_long: bool,
    pub slot: u64,
}

#[event]
pub struct LiquidatePositionEvent {
    pub liquidator: Pubkey,
    pub owner: Pubkey,
    pub position: Pubkey,
    pub keeper_reward: u64,
    pub insurance_cut: u64,
    pub slot: u64,
}

#[error_code]
pub enum ErrorCode {
    #[msg("Oracle feed is invalid or unsupported")]
    InvalidOracleFeed,
    #[msg("Oracle price is stale or outside acceptable slot window")]
    StaleOraclePrice,
    #[msg("Oracle confidence interval exceeds protocol tolerance")]
    ExcessiveOracleConfidenceInterval,
    #[msg("Negative oracle price is disallowed")]
    NegativePrice,
    #[msg("Requested position size exceeds maximum allowed leverage")]
    LeverageExceedsMax,
    #[msg("Position is healthy and cannot be liquidated")]
    PositionHealthy,
    #[msg("Position is inactive or already closed")]
    PositionInactive,
    #[msg("Arithmetic overflow occurred")]
    MathOverflow,
}