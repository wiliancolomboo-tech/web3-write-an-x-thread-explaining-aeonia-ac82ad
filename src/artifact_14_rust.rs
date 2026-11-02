use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};

declare_id!("AeonTrd111111111111111111111111111111111111");

pub const PRECISION: u128 = 1_000_000_000_000; // 1e12 Fixed point
pub const MAX_LEVERAGE: u64 = 50; // 50x Max Leverage
pub const MAINTENANCE_MARGIN_BPS: u64 = 500; // 5.00% (500 BPS)
pub const LIQUIDATION_FEE_BPS: u64 = 250; // 2.50% (250 BPS)
pub const PROTOCOL_FEE_BPS: u64 = 10; // 0.10% (10 BPS)
pub const MAX_ORACLE_STALENESS_SLOTS: u64 = 3;

#[program]
pub mod aeonian_trade {
    use super::*;

    pub fn initialize_market(
        ctx: Context<InitializeMarket>,
        market_id: u64,
        initial_funding_rate: i64,
    ) -> Result<()> {
        let market = &mut ctx.accounts.market.load_init()?;
        market.authority = ctx.accounts.authority.key();
        market.collateral_mint = ctx.accounts.collateral_mint.key();
        market.vault = ctx.accounts.vault.key();
        market.market_id = market_id;
        market.total_long_size = 0;
        market.total_short_size = 0;
        market.cumulative_funding_rate_long = 0;
        market.cumulative_funding_rate_short = 0;
        market.last_funding_slot = Clock::get()?.slot;
        market.last_funding_rate = initial_funding_rate;
        market.bump = ctx.bumps.market;
        market.vault_bump = ctx.bumps.vault;

        msg!("Aeonian Market Initialized: ID {}", market_id);
        Ok(())
    }

    pub fn open_position(
        ctx: Context<OpenPosition>,
        size: u64,
        collateral_amount: u64,
        is_long: bool,
        oracle_price: u64,
        oracle_slot: u64,
    ) -> Result<()> {
        let current_slot = Clock::get()?.slot;
        require!(
            current_slot.saturating_sub(oracle_slot) <= MAX_ORACLE_STALENESS_SLOTS,
            AeonianError::StaleOraclePrice
        );
        require!(collateral_amount > 0, AeonianError::ZeroCollateral);
        require!(size > 0, AeonianError::ZeroPositionSize);

        // Verify Leverage Bound: size <= collateral * oracle_price * MAX_LEVERAGE
        let notional_value = (size as u128)
            .checked_mul(oracle_price as u128)
            .ok_or(AeonianError::MathOverflow)?
            .checked_div(PRECISION)
            .ok_or(AeonianError::MathOverflow)?;

        let max_allowed_notional = (collateral_amount as u128)
            .checked_mul(MAX_LEVERAGE as u128)
            .ok_or(AeonianError::MathOverflow)?;

        require!(
            notional_value <= max_allowed_notional,
            AeonianError::ExcessiveLeverage
        );

        // Protocol fee calculation
        let fee = notional_value
            .checked_mul(PROTOCOL_FEE_BPS as u128)
            .ok_or(AeonianError::MathOverflow)?
            .checked_div(10_000)
            .ok_or(AeonianError::MathOverflow)? as u64;

        let net_deposit = collateral_amount
            .checked_sub(fee)
            .ok_or(AeonianError::InsufficientCollateralForFees)?;

        // Transfer collateral into vault
        token::transfer(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.user_token_account.to_account_info(),
                    to: ctx.accounts.vault.to_account_info(),
                    authority: ctx.accounts.user.to_account_info(),
                },
            ),
            collateral_amount,
        )?;

        // Update Market State
        let mut market = ctx.accounts.market.load_mut()?;
        if is_long {
            market.total_long_size = market
                .total_long_size
                .checked_add(size)
                .ok_or(AeonianError::MathOverflow)?;
        } else {
            market.total_short_size = market
                .total_short_size
                .checked_add(size)
                .ok_or(AeonianError::MathOverflow)?;
        }

        // Initialize User Position
        let position = &mut ctx.accounts.position;
        position.owner = ctx.accounts.user.key();
        position.market = ctx.accounts.market.key();
        position.size = size;
        position.collateral = net_deposit;
        position.entry_price = oracle_price;
        position.is_long = is_long;
        position.last_updated_slot = current_slot;
        position.bump = ctx.bumps.position;

        msg!(
            "Position Opened: User={}, Size={}, Collateral={}, IsLong={}",
            ctx.accounts.user.key(),
            size,
            net_deposit,
            is_long
        );
        Ok(())
    }

    pub fn liquidate_position(
        ctx: Context<LiquidatePosition>,
        oracle_price: u64,
        oracle_slot: u64,
    ) -> Result<()> {
        let current_slot = Clock::get()?.slot;
        require!(
            current_slot.saturating_sub(oracle_slot) <= MAX_ORACLE_STALENESS_SLOTS,
            AeonianError::StaleOraclePrice
        );

        let position = &mut ctx.accounts.position;
        require!(position.size > 0, AeonianError::PositionAlreadyClosed);

        let entry_notional = (position.size as u128)
            .checked_mul(position.entry_price as u128)
            .ok_or(AeonianError::MathOverflow)?
            .checked_div(PRECISION)
            .ok_or(AeonianError::MathOverflow)?;

        let current_notional = (position.size as u128)
            .checked_mul(oracle_price as u128)
            .ok_or(AeonianError::MathOverflow)?
            .checked_div(PRECISION)
            .ok_or(AeonianError::MathOverflow)?;

        let pnl: i128 = if position.is_long {
            (current_notional as i128) - (entry_notional as i128)
        } else {
            (entry_notional as i128) - (current_notional as i128)
        };

        let effective_equity: i128 = (position.collateral as i128) + pnl;
        let maintenance_margin = (current_notional
            .checked_mul(MAINTENANCE_MARGIN_BPS as u128)
            .ok_or(AeonianError::MathOverflow)?
            .checked_div(10_000)
            .ok_or(AeonianError::MathOverflow)?) as i128;

        // Position must be insolvent under maintenance threshold
        require!(
            effective_equity <= maintenance_margin,
            AeonianError::PositionNotLiquidatable
        );

        let liquidator_bounty = if effective_equity > 0 {
            let bounty = (effective_equity as u128)
                .checked_mul(LIQUIDATION_FEE_BPS as u128)
                .ok_or(AeonianError::MathOverflow)?
                .checked_div(10_000)
                .ok_or(AeonianError::MathOverflow)? as u64;
            std::cmp::min(bounty, position.collateral)
        } else {
            0
        };

        let market_key = ctx.accounts.market.key();
        let vault_seeds = &[
            b"vault".as_ref(),
            market_key.as_ref(),
            &[ctx.accounts.market.load()?.vault_bump],
        ];
        let signer_seeds = &[&vault_seeds[..]];

        if liquidator_bounty > 0 {
            token::transfer(
                CpiContext::new_with_signer(
                    ctx.accounts.token_program.to_account_info(),
                    Transfer {
                        from: ctx.accounts.vault.to_account_info(),
                        to: ctx.accounts.liquidator_token_account.to_account_info(),
                        authority: ctx.accounts.vault.to_account_info(),
                    },
                    signer_seeds,
                ),
                liquidator_bounty,
            )?;
        }

        // Decrement Market open interest
        let mut market = ctx.accounts.market.load_mut()?;
        if position.is_long {
            market.total_long_size = market.total_long_size.saturating_sub(position.size);
        } else {
            market.total_short_size = market.total_short_size.saturating_sub(position.size);
        }

        // Reset position state
        position.size = 0;
        position.collateral = 0;
        position.entry_price = 0;

        msg!("Position Liquidated: Bounty Paid={}", liquidator_bounty);
        Ok(())
    }
}

// -----------------------------------------------------------------------------
// Account Contexts
// -----------------------------------------------------------------------------

#[derive(Accounts)]
#[instruction(market_id: u64)]
pub struct InitializeMarket<'info> {
    #[account(
        init,
        payer = authority,
        space = 8 + std::mem::size_of::<MarketState>(),
        seeds = [b"market", collateral_mint.key().as_ref(), &market_id.to_le_bytes()],
        bump
    )]
    pub market: AccountLoader<'info, MarketState>,

    #[account(
        init,
        payer = authority,
        seeds = [b"vault", market.key().as_ref()],
        bump,
        token::mint = collateral_mint,
        token::authority = vault
    )]
    pub vault: Account<'info, TokenAccount>,

    pub collateral_mint: Account<'info, Mint>,

    #[account(mut)]
    pub authority: Signer<'info>,
    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct OpenPosition<'info> {
    #[account(mut)]
    pub market: AccountLoader<'info, MarketState>,

    #[account(
        init_if_needed,
        payer = user,
        space = 8 + PositionState::LEN,
        seeds = [b"position", market.key().as_ref(), user.key().as_ref()],
        bump
    )]
    pub position: Account<'info, PositionState>,

    #[account(
        mut,
        seeds = [b"vault", market.key().as_ref()],
        bump = market.load()?.vault_bump
    )]
    pub vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = user_token_account.owner == user.key(),
        constraint = user_token_account.mint == market.load()?.collateral_mint
    )]
    pub user_token_account: Account<'info, TokenAccount>,

    #[account(mut)]
    pub user: Signer<'info>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct LiquidatePosition<'info> {
    #[account(mut)]
    pub market: AccountLoader<'info, MarketState>,

    #[account(
        mut,
        seeds = [b"position", market.key().as_ref(), position.owner.as_ref()],
        bump = position.bump
    )]
    pub position: Account<'info, PositionState>,

    #[account(
        mut,
        seeds = [b"vault", market.key().as_ref()],
        bump = market.load()?.vault_bump
    )]
    pub vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = liquidator_token_account.mint == market.load()?.collateral_mint
    )]
    pub liquidator_token_account: Account<'info, TokenAccount>,

    #[account(mut)]
    pub liquidator: Signer<'info>,
    pub token_program: Program<'info, Token>,
}

// -----------------------------------------------------------------------------
// State Structs
// -----------------------------------------------------------------------------

#[account(zero_copy)]
#[derive(Default)]
#[repr(C)]
pub struct MarketState {
    pub authority: Pubkey,
    pub collateral_mint: Pubkey,
    pub vault: Pubkey,
    pub market_id: u64,
    pub total_long_size: u64,
    pub total_short_size: u64,
    pub cumulative_funding_rate_long: i128,
    pub cumulative_funding_rate_short: i128,
    pub last_funding_slot: u64,
    pub last_funding_rate: i64,
    pub bump: u8,
    pub vault_bump: u8,
    pub _padding: [u8; 6],
}

#[account]
#[derive(Default)]
pub struct PositionState {
    pub owner: Pubkey,
    pub market: Pubkey,
    pub size: u64,
    pub collateral: u64,
    pub entry_price: u64,
    pub is_long: bool,
    pub last_updated_slot: u64,
    pub bump: u8,
}

impl PositionState {
    pub const LEN: usize = 32 + 32 + 8 + 8 + 8 + 1 + 8 + 1;
}

// -----------------------------------------------------------------------------
// Errors
// -----------------------------------------------------------------------------

#[error_code]
pub enum AeonianError {
    #[msg("Supplied oracle price is outside allowable slot latency.")]
    StaleOraclePrice,
    #[msg("Collateral amount cannot be zero.")]
    ZeroCollateral,
    #[msg("Position size cannot be zero.")]
    ZeroPositionSize,
    #[msg("Requested position exceeds maximum allowed leverage.")]
    ExcessiveLeverage,
    #[msg("Collateral is insufficient to cover upfront fees.")]
    InsufficientCollateralForFees,
    #[msg("Arithmetic overflow or underflow occurred.")]
    MathOverflow,
    #[msg("Position is not eligible for liquidation.")]
    PositionNotLiquidatable,
    #[msg("Position is already closed.")]
    PositionAlreadyClosed,
}