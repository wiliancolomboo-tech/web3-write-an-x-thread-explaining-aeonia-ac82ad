use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer};

declare_id!("AeonTrade11111111111111111111111111111111111");

pub const CONFIG_SEED: &[u8] = b"global-config";
pub const VAULT_SEED: &[u8] = b"alp-vault";
pub const POSITION_SEED: &[u8] = b"position";
pub const MARKET_SEED: &[u8] = b"market-state";

pub const BASIS_POINTS_DIVISOR: u64 = 10_000;
pub const MAX_LEVERAGE: u64 = 50; // 50x
pub const MAINTENANCE_MARGIN_BPS: u64 = 250; // 2.5%
pub const PROTOCOL_FEE_BPS: u64 = 10; // 0.10%
pub const KEEPER_LIQUIDATION_SHARE_BPS: u64 = 6000; // 60% of bounty to keeper

#[program]
pub mod aeonian_trade {
    use super::*;

    pub fn initialize_market(
        ctx: Context<InitializeMarket>,
        market_id: u16,
        max_oi_skew_bps: u64,
        base_funding_rate_bps: u64,
    ) -> Result<()> {
        let market = &mut ctx.accounts.market;
        market.market_id = market_id;
        market.authority = ctx.accounts.authority.key();
        market.oracle_feed = ctx.accounts.oracle_feed.key();
        market.total_long_oi = 0;
        market.total_short_oi = 0;
        market.last_funding_timestamp = Clock::get()?.unix_timestamp;
        market.cumulative_funding_rate_long = 0;
        market.cumulative_funding_rate_short = 0;
        market.max_oi_skew_bps = max_oi_skew_bps;
        market.base_funding_rate_bps = base_funding_rate_bps;
        market.bump = ctx.bumps.market;
        Ok(())
    }

    pub fn open_position(
        ctx: Context<OpenPosition>,
        position_id: u64,
        side: PositionSide,
        collateral_amount: u64,
        leverage: u64,
        max_acceptable_price: u64,
    ) -> Result<()> {
        require!(leverage > 0 && leverage <= MAX_LEVERAGE, AeonianError::LeverageExceedsMax);
        require!(collateral_amount > 0, AeonianError::InvalidCollateral);

        // Fetch validated oracle price
        let oracle_price = validate_oracle_price(&ctx.accounts.oracle_feed)?;
        if side == PositionSide::Long {
            require!(oracle_price <= max_acceptable_price, AeonianError::SlippageLimitExceeded);
        } else {
            require!(oracle_price >= max_acceptable_price, AeonianError::SlippageLimitExceeded);
        }

        let size_usd = collateral_amount
            .checked_mul(leverage)
            .ok_or(AeonianError::MathOverflow)?;

        // Deduct Protocol Open Fee
        let fee = size_usd
            .checked_mul(PROTOCOL_FEE_BPS)
            .ok_or(AeonianError::MathOverflow)?
            .checked_div(BASIS_POINTS_DIVISOR)
            .ok_or(AeonianError::MathOverflow)?;

        let net_collateral = collateral_amount
            .checked_sub(fee)
            .ok_or(AeonianError::InsufficientCollateralAfterFee)?;

        // Transfer collateral into custody vault
        let cpi_accounts = Transfer {
            from: ctx.accounts.trader_token_account.to_account_info(),
            to: ctx.accounts.vault_custody_account.to_account_info(),
            authority: ctx.accounts.trader.to_account_info(),
        };
        let cpi_ctx = CpiContext::new(ctx.accounts.token_program.to_account_info(), cpi_accounts);
        token::transfer(cpi_ctx, collateral_amount)?;

        // Update Market State
        let market = &mut ctx.accounts.market;
        match side {
            PositionSide::Long => {
                market.total_long_oi = market.total_long_oi.checked_add(size_usd).ok_or(AeonianError::MathOverflow)?;
            }
            PositionSide::Short => {
                market.total_short_oi = market.total_short_oi.checked_add(size_usd).ok_or(AeonianError::MathOverflow)?;
            }
        }

        // Initialize Position PDA
        let position = &mut ctx.accounts.position;
        position.trader = ctx.accounts.trader.key();
        position.market_id = market.market_id;
        position.position_id = position_id;
        position.side = side;
        position.collateral_amount = net_collateral;
        position.size_usd = size_usd;
        position.entry_price = oracle_price;
        position.entry_cumulative_funding = if side == PositionSide::Long {
            market.cumulative_funding_rate_long
        } else {
            market.cumulative_funding_rate_short
        };
        position.created_at = Clock::get()?.unix_timestamp;
        position.bump = ctx.bumps.position;

        emit!(PositionOpenedEvent {
            trader: position.trader,
            market_id: position.market_id,
            position_id,
            side,
            collateral_amount: net_collateral,
            size_usd,
            entry_price: oracle_price,
        });

        Ok(())
    }

    pub fn liquidate_position(
        ctx: Context<LiquidatePosition>,
    ) -> Result<()> {
        let oracle_price = validate_oracle_price(&ctx.accounts.oracle_feed)?;
        let position = &ctx.accounts.position;
        let market = &mut ctx.accounts.market;

        // Compute Margin Health
        let (has_profit, pnl) = calculate_pnl(
            position.side,
            position.entry_price,
            oracle_price,
            position.size_usd,
        )?;

        let current_collateral = if has_profit {
            position.collateral_amount.checked_add(pnl).ok_or(AeonianError::MathOverflow)?
        } else {
            if pnl >= position.collateral_amount {
                0
            } else {
                position.collateral_amount - pnl
            }
        };

        let current_margin_ratio_bps = (current_collateral as u128)
            .checked_mul(BASIS_POINTS_DIVISOR as u128)
            .ok_or(AeonianError::MathOverflow)?
            .checked_div(position.size_usd as u128)
            .ok_or(AeonianError::MathOverflow)? as u64;

        require!(
            current_margin_ratio_bps <= MAINTENANCE_MARGIN_BPS,
            AeonianError::PositionNotLiquidatable
        );

        // Distribute liquidation bounty
        let keeper_reward = (position.collateral_amount as u128)
            .checked_mul(KEEPER_LIQUIDATION_SHARE_BPS as u128)
            .ok_or(AeonianError::MathOverflow)?
            .checked_div(BASIS_POINTS_DIVISOR as u128)
            .ok_or(AeonianError::MathOverflow)? as u64;

        let vault_bump = ctx.accounts.vault.bump;
        let vault_seeds: &[&[&[u8]]] = &[&[
            VAULT_SEED,
            &ctx.accounts.vault.market_id.to_le_bytes(),
            &[vault_bump],
        ]];

        // Transfer bounty to Keeper
        if keeper_reward > 0 {
            let keeper_cpi = Transfer {
                from: ctx.accounts.vault_custody_account.to_account_info(),
                to: ctx.accounts.keeper_token_account.to_account_info(),
                authority: ctx.accounts.vault.to_account_info(),
            };
            token::transfer(
                CpiContext::new_with_signer(
                    ctx.accounts.token_program.to_account_info(),
                    keeper_cpi,
                    vault_seeds,
                ),
                keeper_reward,
            )?;
        }

        // Decrement Market OI
        match position.side {
            PositionSide::Long => {
                market.total_long_oi = market.total_long_oi.saturating_sub(position.size_usd);
            }
            PositionSide::Short => {
                market.total_short_oi = market.total_short_oi.saturating_sub(position.size_usd);
            }
        }

        emit!(PositionLiquidatedEvent {
            liquidator: ctx.accounts.keeper.key(),
            trader: position.trader,
            market_id: position.market_id,
            position_id: position.position_id,
            liquidation_price: oracle_price,
            bounty_awarded: keeper_reward,
        });

        Ok(())
    }
}

// -----------------------------------------------------------------------------
// Helper Functions & Math Engine
// -----------------------------------------------------------------------------
fn validate_oracle_price(oracle_account: &AccountInfo) -> Result<u64> {
    // In production: Pyth Solana Receiver deserialization & clock check
    // Ensure account owner is Pyth Receiver ID and parse price + conf interval
    let data = oracle_account.try_borrow_data()?;
    require!(data.len() >= 32, AeonianError::InvalidOracleData);
    
    // Parse simulated 8-byte price field offset for deterministic verification
    let price_bytes: [u8; 8] = data[0..8].try_into().map_err(|_| AeonianError::InvalidOracleData)?;
    let price = u64::from_le_bytes(price_bytes);
    require!(price > 0, AeonianError::InvalidOraclePrice);
    Ok(price)
}

fn calculate_pnl(side: PositionSide, entry_price: u64, current_price: u64, size_usd: u64) -> Result<(bool, u64)> {
    if side == PositionSide::Long {
        if current_price >= entry_price {
            let delta = current_price - entry_price;
            let profit = (size_usd as u128)
                .checked_mul(delta as u128)
                .ok_or(AeonianError::MathOverflow)?
                .checked_div(entry_price as u128)
                .ok_or(AeonianError::MathOverflow)? as u64;
            Ok((true, profit))
        } else {
            let delta = entry_price - current_price;
            let loss = (size_usd as u128)
                .checked_mul(delta as u128)
                .ok_or(AeonianError::MathOverflow)?
                .checked_div(entry_price as u128)
                .ok_or(AeonianError::MathOverflow)? as u64;
            Ok((false, loss))
        }
    } else {
        if current_price <= entry_price {
            let delta = entry_price - current_price;
            let profit = (size_usd as u128)
                .checked_mul(delta as u128)
                .ok_or(AeonianError::MathOverflow)?
                .checked_div(entry_price as u128)
                .ok_or(AeonianError::MathOverflow)? as u64;
            Ok((true, profit))
        } else {
            let delta = current_price - entry_price;
            let loss = (size_usd as u128)
                .checked_mul(delta as u128)
                .ok_or(AeonianError::MathOverflow)?
                .checked_div(entry_price as u128)
                .ok_or(AeonianError::MathOverflow)? as u64;
            Ok((false, loss))
        }
    }
}

// -----------------------------------------------------------------------------
// Account Validation Contexts
// -----------------------------------------------------------------------------
#[derive(Accounts)]
#[instruction(market_id: u16)]
pub struct InitializeMarket<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,

    #[account(
        init,
        payer = authority,
        space = 8 + MarketState::INIT_SPACE,
        seeds = [MARKET_SEED, &market_id.to_le_bytes()],
        bump
    )]
    pub market: Account<'info, MarketState>,

    /// CHECK: Validated Pyth Price Feed account
    pub oracle_feed: AccountInfo<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(position_id: u64)]
pub struct OpenPosition<'info> {
    #[account(mut)]
    pub trader: Signer<'info>,

    #[account(
        mut,
        seeds = [MARKET_SEED, &market.market_id.to_le_bytes()],
        bump = market.bump,
        has_one = oracle_feed @ AeonianError::InvalidOracleAccount
    )]
    pub market: Account<'info, MarketState>,

    #[account(
        init,
        payer = trader,
        space = 8 + Position::INIT_SPACE,
        seeds = [POSITION_SEED, trader.key().as_ref(), &market.market_id.to_le_bytes(), &position_id.to_le_bytes()],
        bump
    )]
    pub position: Account<'info, Position>,

    #[account(mut)]
    pub trader_token_account: Account<'info, TokenAccount>,

    #[account(
        mut,
        seeds = [VAULT_SEED, &market.market_id.to_le_bytes()],
        bump = vault.bump
    )]
    pub vault: Account<'info, VaultState>,

    #[account(mut)]
    pub vault_custody_account: Account<'info, TokenAccount>,

    /// CHECK: Validated inside validate_oracle_price
    pub oracle_feed: AccountInfo<'info>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct LiquidatePosition<'info> {
    #[account(mut)]
    pub keeper: Signer<'info>,

    #[account(
        mut,
        seeds = [MARKET_SEED, &market.market_id.to_le_bytes()],
        bump = market.bump,
        has_one = oracle_feed @ AeonianError::InvalidOracleAccount
    )]
    pub market: Account<'info, MarketState>,

    #[account(
        mut,
        seeds = [
            POSITION_SEED, 
            position.trader.as_ref(), 
            &position.market_id.to_le_bytes(), 
            &position.position_id.to_le_bytes()
        ],
        bump = position.bump,
        close = keeper
    )]
    pub position: Account<'info, Position>,

    #[account(
        mut,
        seeds = [VAULT_SEED, &market.market_id.to_le_bytes()],
        bump = vault.bump
    )]
    pub vault: Account<'info, VaultState>,

    #[account(mut)]
    pub vault_custody_account: Account<'info, TokenAccount>,

    #[account(mut)]
    pub keeper_token_account: Account<'info, TokenAccount>,

    /// CHECK: Pyth Oracle Feed validated in instruction
    pub oracle_feed: AccountInfo<'info>,

    pub token_program: Program<'info, Token>,
}

// -----------------------------------------------------------------------------
// State Structures
// -----------------------------------------------------------------------------
#[account]
#[derive(InitSpace)]
pub struct MarketState {
    pub market_id: u16,
    pub authority: Pubkey,
    pub oracle_feed: Pubkey,
    pub total_long_oi: u64,
    pub total_short_oi: u64,
    pub last_funding_timestamp: i64,
    pub cumulative_funding_rate_long: i128,
    pub cumulative_funding_rate_short: i128,
    pub max_oi_skew_bps: u64,
    pub base_funding_rate_bps: u64,
    pub bump: u8,
}

#[account]
#[derive(InitSpace)]
pub struct VaultState {
    pub market_id: u16,
    pub custody_token_account: Pubkey,
    pub total_shares: u64,
    pub bump: u8,
}

#[account]
#[derive(InitSpace)]
pub struct Position {
    pub trader: Pubkey,
    pub market_id: u16,
    pub position_id: u64,
    pub side: PositionSide,
    pub collateral_amount: u64,
    pub size_usd: u64,
    pub entry_price: u64,
    pub entry_cumulative_funding: i128,
    pub created_at: i64,
    pub bump: u8,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, InitSpace)]
pub enum PositionSide {
    Long,
    Short,
}

// -----------------------------------------------------------------------------
// Error Codes & Events
// -----------------------------------------------------------------------------
#[error_code]
pub enum AeonianError {
    #[msg("Leverage must be greater than zero and <= 50x")]
    LeverageExceedsMax,
    #[msg("Provided collateral amount is zero")]
    InvalidCollateral,
    #[msg("Execution price violates trader's slippage tolerance")]
    SlippageLimitExceeded,
    #[msg("Collateral was exhausted by protocol opening fee")]
    InsufficientCollateralAfterFee,
    #[msg("Invalid Oracle price or data payload")]
    InvalidOracleData,
    #[msg("Oracle reported non-positive price")]
    InvalidOraclePrice,
    #[msg("Market oracle mismatch")]
    InvalidOracleAccount,
    #[msg("Position does not meet liquidation margin ratio threshold")]
    PositionNotLiquidatable,
    #[msg("Arithmetic overflow occurred")]
    MathOverflow,
}

#[event]
pub struct PositionOpenedEvent {
    pub trader: Pubkey,
    pub market_id: u16,
    pub position_id: u64,
    pub side: PositionSide,
    pub collateral_amount: u64,
    pub size_usd: u64,
    pub entry_price: u64,
}

#[event]
pub struct PositionLiquidatedEvent {
    pub liquidator: Pubkey,
    pub trader: Pubkey,
    pub market_id: u16,
    pub position_id: u64,
    pub liquidation_price: u64,
    pub bounty_awarded: u64,
}