use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};

declare_id!("Aeon111111111111111111111111111111111111111");

pub const CONFIG_SEED: &[u8] = b"global-config";
pub const MARKET_SEED: &[u8] = b"market";
pub const POSITION_SEED: &[u8] = b"position";
pub const VAULT_SEED: &[u8] = b"vault";

pub const PRICE_DECIMALS: u32 = 6;
pub const MAX_LEVERAGE: u8 = 50;
pub const MAINTENANCE_MARGIN_BPS: u64 = 500; // 5.0%
pub const LIQUIDATION_FEE_BPS: u64 = 250;     // 2.5%
pub const MAX_CONF_RATIO_BPS: u64 = 20;       // Max 0.20% price confidence spread

#[program]
pub mod aeonian_trade {
    use super::*;

    pub fn initialize_market(
        ctx: Context<InitializeMarket>,
        initial_mark_price: u64,
        max_leverage: u8,
    ) -> Result<()> {
        require!(max_leverage <= MAX_LEVERAGE, AeonianError::InvalidLeverage);
        require!(initial_mark_price > 0, AeonianError::InvalidPrice);

        let market = &mut ctx.accounts.market;
        market.admin = ctx.accounts.admin.key();
        market.asset_mint = ctx.accounts.asset_mint.key();
        market.vault = ctx.accounts.vault.key();
        market.mark_price = initial_mark_price;
        market.open_interest_long = 0;
        market.open_interest_short = 0;
        market.cumulative_funding_rate = 0;
        market.last_funding_timestamp = Clock::get()?.unix_timestamp;
        market.max_leverage = max_leverage;
        market.bump = ctx.bumps.market;
        market.vault_bump = ctx.bumps.vault;

        msg!("Aeonian Trade: Market initialized for mint {}", market.asset_mint);
        Ok(())
    }

    pub fn open_position(
        ctx: Context<OpenPosition>,
        collateral_amount: u64,
        leverage: u8,
        side: PositionSide,
        simulated_pyth_price: u64,
        simulated_confidence: u64,
    ) -> Result<()> {
        require!(collateral_amount > 0, AeonianError::ZeroDeposit);
        require!(
            leverage > 0 && leverage <= ctx.accounts.market.max_leverage,
            AeonianError::InvalidLeverage
        );

        // Verify oracle integrity: confidence interval <= 0.20% of evaluated price
        let max_allowed_conf = simulated_pyth_price
            .checked_mul(MAX_CONF_RATIO_BPS)
            .ok_or(AeonianError::MathOverflow)?
            .checked_div(10_000)
            .ok_or(AeonianError::MathOverflow)?;
        require!(simulated_confidence <= max_allowed_conf, AeonianError::StaleOrUntrustedOracle);

        // Execute CPI to transfer collateral into the isolated vault
        let transfer_cpi = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.trader_token_account.to_account_info(),
                to: ctx.accounts.vault.to_account_info(),
                authority: ctx.accounts.trader.to_account_info(),
            },
        );
        token::transfer(transfer_cpi, collateral_amount)?;

        // Calculate position size
        let size_usd = (collateral_amount as u128)
            .checked_mul(leverage as u128)
            .ok_or(AeonianError::MathOverflow)?;

        let position = &mut ctx.accounts.position;
        let clock = Clock::get()?;

        position.trader = ctx.accounts.trader.key();
        position.market = ctx.accounts.market.key();
        position.collateral = collateral_amount;
        position.size = size_usd as u64;
        position.entry_price = simulated_pyth_price;
        position.side = side;
        position.last_updated_at = clock.unix_timestamp;
        position.bump = ctx.bumps.position;

        // Mutate Market Aggregations
        let market = &mut ctx.accounts.market;
        match side {
            PositionSide::Long => {
                market.open_interest_long = market
                    .open_interest_long
                    .checked_add(position.size)
                    .ok_or(AeonianError::MathOverflow)?;
            }
            PositionSide::Short => {
                market.open_interest_short = market
                    .open_interest_short
                    .checked_add(position.size)
                    .ok_or(AeonianError::MathOverflow)?;
            }
        }
        market.mark_price = simulated_pyth_price;

        msg!(
            "Position opened: Trader={}, Side={:?}, Size={}, Collateral={}",
            position.trader,
            position.side,
            position.size,
            position.collateral
        );
        Ok(())
    }

    pub fn liquidate_position(
        ctx: Context<LiquidatePosition>,
        current_oracle_price: u64,
    ) -> Result<()> {
        let position = &ctx.accounts.position;
        require!(position.size > 0, AeonianError::PositionAlreadyClosed);

        // Verify maintenance margin health
        let is_underwater = compute_is_liquidatable(
            position.collateral,
            position.size,
            position.entry_price,
            current_oracle_price,
            position.side,
            MAINTENANCE_MARGIN_BPS,
        )?;

        require!(is_underwater, AeonianError::PositionHealthy);

        // Liquidation Bounty Calculation
        let fee_payout = (position.collateral as u128)
            .checked_mul(LIQUIDATION_FEE_BPS as u128)
            .ok_or(AeonianError::MathOverflow)?
            .checked_div(10_000)
            .ok_or(AeonianError::MathOverflow)? as u64;

        let market_key = ctx.accounts.market.key();
        let asset_mint = ctx.accounts.market.asset_mint;
        let seeds = &[
            VAULT_SEED,
            market_key.as_ref(),
            asset_mint.as_ref(),
            &[ctx.accounts.market.vault_bump],
        ];
        let signer = &[&seeds[..]];

        // Disburse liquidator keeper bounty
        let transfer_cpi = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.vault.to_account_info(),
                to: ctx.accounts.keeper_token_account.to_account_info(),
                authority: ctx.accounts.vault.to_account_info(),
            },
            signer,
        );
        token::transfer(transfer_cpi, fee_payout)?;

        // Deduct from Market Open Interest
        let market = &mut ctx.accounts.market;
        match position.side {
            PositionSide::Long => {
                market.open_interest_long = market.open_interest_long.saturating_sub(position.size);
            }
            PositionSide::Short => {
                market.open_interest_short = market.open_interest_short.saturating_sub(position.size);
            }
        }

        msg!(
            "Liquidation invoked: Keeper={}, Trader={}, BountyDisbursed={}",
            ctx.accounts.keeper.key(),
            position.trader,
            fee_payout
        );
        Ok(())
    }
}

pub fn compute_is_liquidatable(
    collateral: u64,
    size: u64,
    entry_price: u64,
    current_price: u64,
    side: PositionSide,
    maintenance_margin_bps: u64,
) -> Result<bool> {
    let req_maintenance_margin = (size as u128)
        .checked_mul(maintenance_margin_bps as u128)
        .ok_or(AeonianError::MathOverflow)?
        .checked_div(10_000)
        .ok_or(AeonianError::MathOverflow)?;

    let price_delta = if current_price >= entry_price {
        (current_price - entry_price) as u128
    } else {
        (entry_price - current_price) as u128
    };

    let pnl_mag = (size as u128)
        .checked_mul(price_delta)
        .ok_or(AeonianError::MathOverflow)?
        .checked_div(entry_price as u128)
        .ok_or(AeonianError::MathOverflow)?;

    let is_loss = match side {
        PositionSide::Long => current_price < entry_price,
        PositionSide::Short => current_price > entry_price,
    };

    let effective_collateral: i128 = if is_loss {
        (collateral as i128) - (pnl_mag as i128)
    } else {
        (collateral as i128) + (pnl_mag as i128)
    };

    if effective_collateral <= 0 {
        return Ok(true);
    }

    Ok((effective_collateral as u128) < req_maintenance_margin)
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum PositionSide {
    Long,
    Short,
}

#[account]
pub struct Market {
    pub admin: Pubkey,
    pub asset_mint: Pubkey,
    pub vault: Pubkey,
    pub mark_price: u64,
    pub open_interest_long: u64,
    pub open_interest_short: u64,
    pub cumulative_funding_rate: i128,
    pub last_funding_timestamp: i64,
    pub max_leverage: u8,
    pub bump: u8,
    pub vault_bump: u8,
}

impl Market {
    pub const LEN: usize = 8 + 32 + 32 + 32 + 8 + 8 + 8 + 16 + 8 + 1 + 1 + 1 + 64;
}

#[account]
pub struct Position {
    pub trader: Pubkey,
    pub market: Pubkey,
    pub collateral: u64,
    pub size: u64,
    pub entry_price: u64,
    pub side: PositionSide,
    pub last_updated_at: i64,
    pub bump: u8,
}

impl Position {
    pub const LEN: usize = 8 + 32 + 32 + 8 + 8 + 8 + 1 + 8 + 1 + 32;
}

#[derive(Accounts)]
pub struct InitializeMarket<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,
    pub asset_mint: Account<'info, Mint>,

    #[account(
        init,
        payer = admin,
        space = Market::LEN,
        seeds = [MARKET_SEED, asset_mint.key().as_ref()],
        bump
    )]
    pub market: Account<'info, Market>,

    #[account(
        init,
        payer = admin,
        seeds = [VAULT_SEED, market.key().as_ref(), asset_mint.key().as_ref()],
        bump,
        token::mint = asset_mint,
        token::authority = vault
    )]
    pub vault: Account<'info, TokenAccount>,

    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct OpenPosition<'info> {
    #[account(mut)]
    pub trader: Signer<'info>,

    #[account(
        mut,
        seeds = [MARKET_SEED, market.asset_mint.as_ref()],
        bump = market.bump
    )]
    pub market: Account<'info, Market>,

    #[account(
        init_if_needed,
        payer = trader,
        space = Position::LEN,
        seeds = [POSITION_SEED, trader.key().as_ref(), market.key().as_ref()],
        bump
    )]
    pub position: Account<'info, Position>,

    #[account(
        mut,
        constraint = trader_token_account.owner == trader.key(),
        constraint = trader_token_account.mint == market.asset_mint
    )]
    pub trader_token_account: Account<'info, TokenAccount>,

    #[account(
        mut,
        seeds = [VAULT_SEED, market.key().as_ref(), market.asset_mint.as_ref()],
        bump = market.vault_bump
    )]
    pub vault: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct LiquidatePosition<'info> {
    #[account(mut)]
    pub keeper: Signer<'info>,

    #[account(
        mut,
        seeds = [MARKET_SEED, market.asset_mint.as_ref()],
        bump = market.bump
    )]
    pub market: Account<'info, Market>,

    #[account(
        mut,
        close = keeper,
        seeds = [POSITION_SEED, position.trader.as_ref(), market.key().as_ref()],
        bump = position.bump
    )]
    pub position: Account<'info, Position>,

    #[account(
        mut,
        seeds = [VAULT_SEED, market.key().as_ref(), market.asset_mint.as_ref()],
        bump = market.vault_bump
    )]
    pub vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = keeper_token_account.mint == market.asset_mint
    )]
    pub keeper_token_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

#[error_code]
pub enum AeonianError {
    #[msg("Requested leverage exceeds system parameter ceiling.")]
    InvalidLeverage,
    #[msg("Input price must be positive non-zero.")]
    InvalidPrice,
    #[msg("Deposit collateral cannot be zero.")]
    ZeroDeposit,
    #[msg("Oracle price confidence bound is too wide or timestamp stale.")]
    StaleOrUntrustedOracle,
    #[msg("Position does not satisfy maintenance margin liquidation bounds.")]
    PositionHealthy,
    #[msg("Position is empty or closed.")]
    PositionAlreadyClosed,
    #[msg("Mathematical calculation overflowed integer boundaries.")]
    MathOverflow,
}