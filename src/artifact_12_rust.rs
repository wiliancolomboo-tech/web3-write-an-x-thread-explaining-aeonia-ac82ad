use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer};

declare_id!("AeoN1anTrade11111111111111111111111111111111");

pub const SCALING_FACTOR: u128 = 1_000_000; // 1e6 precision
pub const MAX_STALENESS_SLOTS: u64 = 5;      // Oracle staleness limit: 5 slots
pub const MAX_CONFIDENCE_RATIO_BPS: u64 = 20; // 0.20% max price deviation

#[program]
pub mod aeonian_trade {
    use super::*;

    pub fn initialize_market(
        ctx: Context<InitializeMarket>,
        market_id: u64,
        maintenance_margin_bps: u64,
        liquidation_fee_bps: u64,
        initial_v_quote: u128,
        initial_v_base: u128,
    ) -> Result<()> {
        let market = &mut ctx.accounts.market;
        market.admin = ctx.accounts.admin.key();
        market.market_id = market_id;
        market.collateral_mint = ctx.accounts.collateral_mint.key();
        market.vault = ctx.accounts.vault.key();
        market.maintenance_margin_bps = maintenance_margin_bps;
        market.liquidation_fee_bps = liquidation_fee_bps;
        market.v_quote_reserve = initial_v_quote;
        market.v_base_reserve = initial_v_base;
        market.total_open_interest_long = 0;
        market.total_open_interest_short = 0;
        market.last_funding_slot = Clock::get()?.slot;
        market.cumulative_funding_rate = 0;
        market.bump = ctx.bumps.market;
        market.vault_bump = ctx.bumps.vault;

        emit!(MarketInitialized {
            market_id,
            v_quote_reserve: initial_v_quote,
            v_base_reserve: initial_v_base,
        });

        Ok(())
    }

    pub fn deposit_collateral(ctx: Context<DepositCollateral>, amount: u64) -> Result<()> {
        require!(amount > 0, AeonianError::InvalidAmount);

        let user_margin = &mut ctx.accounts.user_margin;
        if user_margin.owner == Pubkey::default() {
            user_margin.owner = ctx.accounts.user.key();
            user_margin.market = ctx.accounts.market.key();
            user_margin.bump = ctx.bumps.user_margin;
        }

        let cpi_ctx = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.user_token_account.to_account_info(),
                to: ctx.accounts.vault.to_account_info(),
                authority: ctx.accounts.user.to_account_info(),
            },
        );
        token::transfer(cpi_ctx, amount)?;

        user_margin.deposited_collateral = user_margin
            .deposited_collateral
            .checked_add(amount)
            .ok_or(AeonianError::MathOverflow)?;

        emit!(CollateralDeposited {
            user: ctx.accounts.user.key(),
            amount,
            new_balance: user_margin.deposited_collateral,
        });

        Ok(())
    }

    pub fn open_position(
        ctx: Context<OpenPosition>,
        is_long: bool,
        size_quote: u64,
        max_slippage_base: u64,
    ) -> Result<()> {
        require!(size_quote > 0, AeonianError::InvalidAmount);

        let clock = Clock::get()?;
        let market = &mut ctx.accounts.market;
        let position = &mut ctx.accounts.position;
        let user_margin = &mut ctx.accounts.user_margin;

        // Oracle staleness and integrity validation
        let oracle_price = validate_oracle_feed(
            &ctx.accounts.pyth_oracle_feed,
            clock.slot,
        )?;

        // Update funding cumulative accumulator
        sync_funding_rate(market, oracle_price, clock.slot)?;

        // Execute deterministic vAMM constant product swap: x * y = k
        let base_amount = execute_vamm_swap(market, size_quote as u128, is_long)?;
        require!(
            base_amount >= max_slippage_base as u128,
            AeonianError::SlippageExceeded
        );

        // Account for position state
        if position.owner == Pubkey::default() {
            position.owner = ctx.accounts.user.key();
            position.market = market.key();
            position.bump = ctx.bumps.position;
            position.is_long = is_long;
        } else {
            require!(position.is_long == is_long, AeonianError::CannotFlipDirectionDirectly);
        }

        position.size_base = position
            .size_base
            .checked_add(base_amount)
            .ok_or(AeonianError::MathOverflow)?;
        position.notional_quote_entry = position
            .notional_quote_entry
            .checked_add(size_quote)
            .ok_or(AeonianError::MathOverflow)?;
        position.last_cumulative_funding = market.cumulative_funding_rate;

        // Solvency assertion (Leverage & Maintenance check)
        let is_healthy = check_account_health(user_margin, position, oracle_price, market.maintenance_margin_bps)?;
        require!(is_healthy, AeonianError::InsufficientMargin);

        emit!(PositionOpened {
            user: ctx.accounts.user.key(),
            market_id: market.market_id,
            is_long,
            size_quote,
            size_base: base_amount,
            oracle_price,
        });

        Ok(())
    }

    pub fn liquidate_position(ctx: Context<LiquidatePosition>) -> Result<()> {
        let clock = Clock::get()?;
        let market = &mut ctx.accounts.market;
        let position = &mut ctx.accounts.position;
        let user_margin = &mut ctx.accounts.user_margin;

        let oracle_price = validate_oracle_feed(
            &ctx.accounts.pyth_oracle_feed,
            clock.slot,
        )?;

        sync_funding_rate(market, oracle_price, clock.slot)?;

        let is_healthy = check_account_health(user_margin, position, oracle_price, market.maintenance_margin_bps)?;
        require!(!is_healthy, AeonianError::PositionHealthy);

        // Execute Liquidation
        let liquidation_reward = (user_margin.deposited_collateral as u128)
            .checked_mul(market.liquidation_fee_bps as u128)
            .ok_or(AeonianError::MathOverflow)?
            .checked_div(10_000)
            .ok_or(AeonianError::MathOverflow)? as u64;

        let protocol_residual = user_margin.deposited_collateral
            .checked_sub(liquidation_reward)
            .ok_or(AeonianError::MathOverflow)?;

        // Transfer liquidator bounty via CPI from PDA vault
        let market_id_bytes = market.market_id.to_le_bytes();
        let seeds: &[&[&[u8]]] = &[&[
            b"vault",
            market_id_bytes.as_ref(),
            &[market.vault_bump],
        ]];

        let cpi_to_liquidator = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.vault.to_account_info(),
                to: ctx.accounts.liquidator_token_account.to_account_info(),
                authority: ctx.accounts.vault.to_account_info(),
            },
            seeds,
        );
        token::transfer(cpi_to_liquidator, liquidation_reward)?;

        // Residual collateral routes to protocol insurance fund
        let cpi_to_insurance = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.vault.to_account_info(),
                to: ctx.accounts.insurance_vault.to_account_info(),
                authority: ctx.accounts.vault.to_account_info(),
            },
            seeds,
        );
        token::transfer(cpi_to_insurance, protocol_residual)?;

        // Neutralize position
        position.size_base = 0;
        position.notional_quote_entry = 0;
        user_margin.deposited_collateral = 0;

        emit!(PositionLiquidated {
            liquidator: ctx.accounts.liquidator.key(),
            user: position.owner,
            liquidation_reward,
            oracle_price,
        });

        Ok(())
    }
}

// -----------------------------------------------------------------------------
// INTERNAL HELPER LOGIC
// -----------------------------------------------------------------------------

pub fn validate_oracle_feed(feed: &AccountInfo, current_slot: u64) -> Result<u64> {
    // Zero-overhead raw parsing of standardized Pyth Price Feed structure
    let data = feed.try_borrow_data()?;
    require!(data.len() >= 208, AeonianError::InvalidOracleData);

    // Byte offsets per Pyth Price Account Schema:
    // Slot publish time: offset [112..120]
    // Price value: offset [208..216]
    // Confidence: offset [216..224]
    let publish_slot = u64::from_le_bytes(data[112..120].try_into().unwrap());
    let price_raw = i64::from_le_bytes(data[208..216].try_into().unwrap());
    let conf_raw = u64::from_le_bytes(data[216..224].try_into().unwrap());

    require!(price_raw > 0, AeonianError::InvalidOraclePrice);
    require!(
        current_slot.saturating_sub(publish_slot) <= MAX_STALENESS_SLOTS,
        AeonianError::StaleOraclePrice
    );

    let price = price_raw as u64;
    let conf_ratio_bps = conf_raw
        .checked_mul(10_000)
        .ok_or(AeonianError::MathOverflow)?
        .checked_div(price)
        .ok_or(AeonianError::MathOverflow)?;

    require!(
        conf_ratio_bps <= MAX_CONFIDENCE_RATIO_BPS,
        AeonianError::ConfidenceIntervalTooWide
    );

    Ok(price)
}

pub fn execute_vamm_swap(market: &mut Account<MarketState>, quote_in: u128, is_long: bool) -> Result<u128> {
    let k = market.v_quote_reserve
        .checked_mul(market.v_base_reserve)
        .ok_or(AeonianError::MathOverflow)?;

    if is_long {
        let new_quote_reserve = market.v_quote_reserve
            .checked_add(quote_in)
            .ok_or(AeonianError::MathOverflow)?;
        let new_base_reserve = k
            .checked_div(new_quote_reserve)
            .ok_or(AeonianError::MathOverflow)?;
        let base_out = market.v_base_reserve
            .checked_sub(new_base_reserve)
            .ok_or(AeonianError::MathOverflow)?;

        market.v_quote_reserve = new_quote_reserve;
        market.v_base_reserve = new_base_reserve;
        Ok(base_out)
    } else {
        let new_quote_reserve = market.v_quote_reserve
            .checked_sub(quote_in)
            .ok_or(AeonianError::MathOverflow)?;
        let new_base_reserve = k
            .checked_div(new_quote_reserve)
            .ok_or(AeonianError::MathOverflow)?;
        let base_in = new_base_reserve
            .checked_sub(market.v_base_reserve)
            .ok_or(AeonianError::MathOverflow)?;

        market.v_quote_reserve = new_quote_reserve;
        market.v_base_reserve = new_base_reserve;
        Ok(base_in)
    }
}

pub fn sync_funding_rate(market: &mut Account<MarketState>, oracle_price: u64, current_slot: u64) -> Result<()> {
    let slots_elapsed = current_slot.saturating_sub(market.last_funding_slot);
    if slots_elapsed == 0 {
        return Ok(());
    }

    let mark_price = market.v_quote_reserve
        .checked_mul(SCALING_FACTOR)
        .ok_or(AeonianError::MathOverflow)?
        .checked_div(market.v_base_reserve)
        .ok_or(AeonianError::MathOverflow)?;

    let price_diff = (mark_price as i128).checked_sub(oracle_price as i128).unwrap();
    let slot_funding = price_diff
        .checked_mul(slots_elapsed as i128)
        .ok_or(AeonianError::MathOverflow)?
        .checked_div(100_000)
        .ok_or(AeonianError::MathOverflow)?;

    market.cumulative_funding_rate = market
        .cumulative_funding_rate
        .checked_add(slot_funding)
        .ok_or(AeonianError::MathOverflow)?;
    market.last_funding_slot = current_slot;

    Ok(())
}

pub fn check_account_health(
    margin: &Account<UserMarginAccount>,
    position: &Account<PositionAccount>,
    oracle_price: u64,
    maintenance_bps: u64,
) -> Result<bool> {
    if position.size_base == 0 {
        return Ok(true);
    }

    let current_value = position.size_base
        .checked_mul(oracle_price as u128)
        .ok_or(AeonianError::MathOverflow)?
        .checked_div(SCALING_FACTOR)
        .ok_or(AeonianError::MathOverflow)?;

    let required_margin = current_value
        .checked_mul(maintenance_bps as u128)
        .ok_or(AeonianError::MathOverflow)?
        .checked_div(10_000)
        .ok_or(AeonianError::MathOverflow)?;

    let is_healthy = (margin.deposited_collateral as u128) >= required_margin;
    Ok(is_healthy)
}

// -----------------------------------------------------------------------------
// ACCOUNT STRUCTS & VALIDATIONS
// -----------------------------------------------------------------------------

#[derive(Accounts)]
#[instruction(market_id: u64)]
pub struct InitializeMarket<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,
    pub collateral_mint: Account<'info, anchor_spl::token::Mint>,
    #[account(
        init,
        payer = admin,
        space = 8 + MarketState::LEN,
        seeds = [b"market", market_id.to_le_bytes().as_ref()],
        bump
    )]
    pub market: Account<'info, MarketState>,
    #[account(
        init,
        payer = admin,
        seeds = [b"vault", market_id.to_le_bytes().as_ref()],
        bump,
        token::mint = collateral_mint,
        token::authority = vault
    )]
    pub vault: Account<'info, TokenAccount>,
    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct DepositCollateral<'info> {
    #[account(mut)]
    pub user: Signer<'info>,
    #[account(mut)]
    pub market: Account<'info, MarketState>,
    #[account(
        init_if_needed,
        payer = user,
        space = 8 + UserMarginAccount::LEN,
        seeds = [b"margin", market.key().as_ref(), user.key().as_ref()],
        bump
    )]
    pub user_margin: Account<'info, UserMarginAccount>,
    #[account(
        mut,
        constraint = user_token_account.owner == user.key(),
        constraint = user_token_account.mint == market.collateral_mint
    )]
    pub user_token_account: Account<'info, TokenAccount>,
    #[account(
        mut,
        seeds = [b"vault", market.market_id.to_le_bytes().as_ref()],
        bump = market.vault_bump
    )]
    pub vault: Account<'info, TokenAccount>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct OpenPosition<'info> {
    #[account(mut)]
    pub user: Signer<'info>,
    #[account(mut)]
    pub market: Account<'info, MarketState>,
    #[account(
        mut,
        seeds = [b"margin", market.key().as_ref(), user.key().as_ref()],
        bump = user_margin.bump
    )]
    pub user_margin: Account<'info, UserMarginAccount>,
    #[account(
        init_if_needed,
        payer = user,
        space = 8 + PositionAccount::LEN,
        seeds = [b"position", market.key().as_ref(), user.key().as_ref()],
        bump
    )]
    pub position: Account<'info, PositionAccount>,
    /// CHECK: Pyth Oracle Feed validated in handler via raw byte parsing
    pub pyth_oracle_feed: AccountInfo<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct LiquidatePosition<'info> {
    #[account(mut)]
    pub liquidator: Signer<'info>,
    #[account(mut)]
    pub market: Account<'info, MarketState>,
    #[account(
        mut,
        seeds = [b"margin", market.key().as_ref(), position.owner.as_ref()],
        bump = user_margin.bump
    )]
    pub user_margin: Account<'info, UserMarginAccount>,
    #[account(
        mut,
        seeds = [b"position", market.key().as_ref(), position.owner.as_ref()],
        bump = position.bump
    )]
    pub position: Account<'info, PositionAccount>,
    /// CHECK: Pyth Oracle account
    pub pyth_oracle_feed: AccountInfo<'info>,
    #[account(
        mut,
        seeds = [b"vault", market.market_id.to_le_bytes().as_ref()],
        bump = market.vault_bump
    )]
    pub vault: Account<'info, TokenAccount>,
    #[account(mut)]
    pub liquidator_token_account: Account<'info, TokenAccount>,
    #[account(mut)]
    pub insurance_vault: Account<'info, TokenAccount>,
    pub token_program: Program<'info, Token>,
}

#[account]
pub struct MarketState {
    pub admin: Pubkey,
    pub market_id: u64,
    pub collateral_mint: Pubkey,
    pub vault: Pubkey,
    pub maintenance_margin_bps: u64,
    pub liquidation_fee_bps: u64,
    pub v_quote_reserve: u128,
    pub v_base_reserve: u128,
    pub total_open_interest_long: u128,
    pub total_open_interest_short: u128,
    pub last_funding_slot: u64,
    pub cumulative_funding_rate: i128,
    pub bump: u8,
    pub vault_bump: u8,
}

impl MarketState {
    pub const LEN: usize = 32 + 8 + 32 + 32 + 8 + 8 + 16 + 16 + 16 + 16 + 8 + 16 + 1 + 1;
}

#[account]
pub struct UserMarginAccount {
    pub owner: Pubkey,
    pub market: Pubkey,
    pub deposited_collateral: u64,
    pub bump: u8,
}

impl UserMarginAccount {
    pub const LEN: usize = 32 + 32 + 8 + 1;
}

#[account]
pub struct PositionAccount {
    pub owner: Pubkey,
    pub market: Pubkey,
    pub is_long: bool,
    pub size_base: u128,
    pub notional_quote_entry: u64,
    pub last_cumulative_funding: i128,
    pub bump: u8,
}

impl PositionAccount {
    pub const LEN: usize = 32 + 32 + 1 + 16 + 8 + 16 + 1;
}

#[error_code]
pub enum AeonianError {
    #[msg("Invalid collateral or size amount.")]
    InvalidAmount,
    #[msg("Arithmetic overflow occurred.")]
    MathOverflow,
    #[msg("Pyth oracle price data is corrupt or invalid length.")]
    InvalidOracleData,
    #[msg("Pyth oracle price is non-positive.")]
    InvalidOraclePrice,
    #[msg("Pyth oracle price feed is stale.")]
    StaleOraclePrice,
    #[msg("Oracle confidence interval exceeds tolerance threshold.")]
    ConfidenceIntervalTooWide,
    #[msg("Position violates maintenance margin requirement.")]
    InsufficientMargin,
    #[msg("Cannot flip position direction without closing existing.")]
    CannotFlipDirectionDirectly,
    #[msg("Position health is above liquidation threshold.")]
    PositionHealthy,
    #[msg("Slippage tolerance exceeded.")]
    SlippageExceeded,
}

#[event]
pub struct MarketInitialized {
    pub market_id: u64,
    pub v_quote_reserve: u128,
    pub v_base_reserve: u128,
}

#[event]
pub struct CollateralDeposited {
    pub user: Pubkey,
    pub amount: u64,
    pub new_balance: u64,
}

#[event]
pub struct PositionOpened {
    pub user: Pubkey,
    pub market_id: u64,
    pub is_long: bool,
    pub size_quote: u64,
    pub size_base: u128,
    pub oracle_price: u64,
}

#[event]
pub struct PositionLiquidated {
    pub liquidator: Pubkey,
    pub user: Pubkey,
    pub liquidation_reward: u64,
    pub oracle_price: u64,
}