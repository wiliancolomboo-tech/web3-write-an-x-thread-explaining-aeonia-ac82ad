use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};

declare_id!("AeonTrd111111111111111111111111111111111111");

pub const PRECISION: u128 = 1_000_000;
pub const MAX_CONF_RATIO: u64 = 1500; // 0.15% max oracle confidence bound
pub const MAX_STALENESS_SLOTS: u64 = 30; // Max allowed staleness in slots

#[program]
pub mod aeonian_trade {
    use super::*;

    pub fn initialize_market(
        ctx: Context<InitializeMarket>,
        market_id: u64,
        initial_v_quote: u64,
        initial_v_base: u64,
        maintenance_margin_bps: u16,
    ) -> Result<()> {
        let market = &mut ctx.accounts.market;
        market.admin = ctx.accounts.admin.key();
        market.market_id = market_id;
        market.collateral_mint = ctx.accounts.collateral_mint.key();
        market.vault = ctx.accounts.vault.key();
        market.bump = ctx.bumps.market;
        market.vault_bump = ctx.bumps.vault_authority;
        market.virtual_quote_reserves = initial_v_quote as u128;
        market.virtual_base_reserves = initial_v_base as u128;
        market.total_long_open_interest = 0;
        market.total_short_open_interest = 0;
        market.maintenance_margin_bps = maintenance_margin_bps;
        market.last_oracle_slot = Clock::get()?.slot;

        emit!(MarketInitializedEvent {
            market_id,
            maintenance_margin_bps,
            timestamp: Clock::get()?.unix_timestamp,
        });

        Ok(())
    }

    pub fn deposit_collateral(
        ctx: Context<DepositCollateral>,
        amount: u64,
    ) -> Result<()> {
        require!(amount > 0, AeonianError::InvalidAmount);

        let user_margin = &mut ctx.accounts.user_margin.load_init()?;
        user_margin.owner = ctx.accounts.user.key();
        user_margin.market = ctx.accounts.market.key();
        user_margin.collateral_amount = user_margin
            .collateral_amount
            .checked_add(amount)
            .ok_or(AeonianError::MathOverflow)?;
        user_margin.last_interaction_slot = Clock::get()?.slot;

        let cpi_accounts = Transfer {
            from: ctx.accounts.user_token_account.to_account_info(),
            to: ctx.accounts.vault.to_account_info(),
            authority: ctx.accounts.user.to_account_info(),
        };
        let cpi_ctx = CpiContext::new(ctx.accounts.token_program.to_account_info(), cpi_accounts);
        token::transfer(cpi_ctx, amount)?;

        emit!(CollateralDepositedEvent {
            user: ctx.accounts.user.key(),
            amount,
            slot: Clock::get()?.slot,
        });

        Ok(())
    }

    pub fn open_position(
        ctx: Context<OpenPosition>,
        size: u64,
        is_long: bool,
        max_acceptable_slippage_bps: u16,
    ) -> Result<()> {
        let clock = Clock::get()?;
        let market = &mut ctx.accounts.market;
        let mut user_margin = ctx.accounts.user_margin.load_mut()?;

        require!(size > 0, AeonianError::InvalidPositionSize);
        require!(
            user_margin.last_interaction_slot != clock.slot,
            AeonianError::SingleSlotExecutionLocked
        );

        // Parse Oracle Price & Verify Integrity
        let oracle_price = parse_and_validate_pyth_price(
            &ctx.accounts.pyth_oracle_price.to_account_info(),
            clock.slot,
        )?;

        // Compute Execution Price from vAMM
        let execution_price = compute_vamm_price(
            market.virtual_quote_reserves,
            market.virtual_base_reserves,
            size,
            is_long,
        )?;

        // Slippage validation against Oracle Mark Price
        let slippage = execution_price
            .abs_diff(oracle_price)
            .checked_mul(10_000)
            .ok_or(AeonianError::MathOverflow)?
            .checked_div(oracle_price)
            .ok_or(AeonianError::MathOverflow)?;

        require!(
            slippage <= max_acceptable_slippage_bps as u128,
            AeonianError::SlippageExceeded
        );

        // Update virtual reserves
        if is_long {
            market.virtual_base_reserves = market
                .virtual_base_reserves
                .checked_sub(size as u128)
                .ok_or(AeonianError::InsufficientReserves)?;
            market.total_long_open_interest = market
                .total_long_open_interest
                .checked_add(size)
                .ok_or(AeonianError::MathOverflow)?;
        } else {
            market.virtual_base_reserves = market
                .virtual_base_reserves
                .checked_add(size as u128)
                .ok_or(AeonianError::MathOverflow)?;
            market.total_short_open_interest = market
                .total_short_open_interest
                .checked_add(size)
                .ok_or(AeonianError::MathOverflow)?;
        }

        // Margin requirement validation
        let required_collateral = (size as u128)
            .checked_mul(execution_price)
            .ok_or(AeonianError::MathOverflow)?
            .checked_mul(market.maintenance_margin_bps as u128)
            .ok_or(AeonianError::MathOverflow)?
            .checked_div(10_000 * PRECISION)
            .ok_or(AeonianError::MathOverflow)? as u64;

        require!(
            user_margin.collateral_amount >= required_collateral,
            AeonianError::InsufficientCollateral
        );

        // Update user position
        user_margin.position_size = size;
        user_margin.entry_price = execution_price as u64;
        user_margin.is_long = if is_long { 1 } else { 0 };
        user_margin.last_interaction_slot = clock.slot;

        emit!(PositionOpenedEvent {
            user: user_margin.owner,
            market: market.key(),
            size,
            execution_price: execution_price as u64,
            is_long,
            slot: clock.slot,
        });

        Ok(())
    }

    pub fn crank_liquidation(ctx: Context<CrankLiquidation>) -> Result<()> {
        let clock = Clock::get()?;
        let market = &mut ctx.accounts.market;
        let mut user_margin = ctx.accounts.user_margin.load_mut()?;

        require!(user_margin.position_size > 0, AeonianError::NoActivePosition);

        let oracle_price = parse_and_validate_pyth_price(
            &ctx.accounts.pyth_oracle_price.to_account_info(),
            clock.slot,
        )?;

        // Calculate Equity & Check Maintenance Margin
        let is_long = user_margin.is_long == 1;
        let pnl: i128 = if is_long {
            ((oracle_price as i128) - (user_margin.entry_price as i128))
                .checked_mul(user_margin.position_size as i128)
                .ok_or(AeonianError::MathOverflow)?
                .checked_div(PRECISION as i128)
                .ok_or(AeonianError::MathOverflow)?
        } else {
            ((user_margin.entry_price as i128) - (oracle_price as i128))
                .checked_mul(user_margin.position_size as i128)
                .ok_or(AeonianError::MathOverflow)?
                .checked_div(PRECISION as i128)
                .ok_or(AeonianError::MathOverflow)?
        };

        let current_equity = (user_margin.collateral_amount as i128)
            .checked_add(pnl)
            .ok_or(AeonianError::MathOverflow)?;

        let position_val = (user_margin.position_size as u128)
            .checked_mul(oracle_price)
            .ok_or(AeonianError::MathOverflow)?
            .checked_div(PRECISION)
            .ok_or(AeonianError::MathOverflow)? as u64;

        let maintenance_threshold = (position_val as u128)
            .checked_mul(market.maintenance_margin_bps as u128)
            .ok_or(AeonianError::MathOverflow)?
            .checked_div(10_000)
            .ok_or(AeonianError::MathOverflow)? as i128;

        require!(
            current_equity < maintenance_threshold,
            AeonianError::PositionHealthy
        );

        // Execute liquidation: Distribute liquidator fee (2.5%)
        let liquidator_fee = (user_margin.collateral_amount as u128)
            .checked_mul(250)
            .ok_or(AeonianError::MathOverflow)?
            .checked_div(10_000)
            .ok_or(AeonianError::MathOverflow)? as u64;

        let market_id_bytes = market.market_id.to_le_bytes();
        let authority_seeds = &[
            b"authority",
            market_id_bytes.as_ref(),
            &[market.vault_bump],
        ];
        let signer = &[&authority_seeds[..]];

        let cpi_accounts = Transfer {
            from: ctx.accounts.vault.to_account_info(),
            to: ctx.accounts.liquidator_token_account.to_account_info(),
            authority: ctx.accounts.vault_authority.to_account_info(),
        };
        let cpi_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            cpi_accounts,
            signer,
        );
        token::transfer(cpi_ctx, liquidator_fee)?;

        // Release Open Interest
        if is_long {
            market.total_long_open_interest = market
                .total_long_open_interest
                .saturating_sub(user_margin.position_size);
        } else {
            market.total_short_open_interest = market
                .total_short_open_interest
                .saturating_sub(user_margin.position_size);
        }

        // Reset user margin account
        user_margin.position_size = 0;
        user_margin.entry_price = 0;
        user_margin.collateral_amount = 0;
        user_margin.last_interaction_slot = clock.slot;

        emit!(PositionLiquidatedEvent {
            user: user_margin.owner,
            liquidator: ctx.accounts.liquidator.key(),
            reward: liquidator_fee,
            slot: clock.slot,
        });

        Ok(())
    }
}

// -----------------------------------------------------------------------------
// INTERNAL HELPER FUNCTIONS
// -----------------------------------------------------------------------------
pub fn compute_vamm_price(
    v_quote: u128,
    v_base: u128,
    delta_base: u64,
    is_long: bool,
) -> Result<u128> {
    require!(v_base > 0 && v_quote > 0, AeonianError::ZeroReserves);
    let invariant = v_quote
        .checked_mul(v_base)
        .ok_or(AeonianError::MathOverflow)?;

    if is_long {
        let new_base = v_base
            .checked_sub(delta_base as u128)
            .ok_or(AeonianError::InsufficientReserves)?;
        let new_quote = invariant
            .checked_div(new_base)
            .ok_or(AeonianError::MathOverflow)?;
        let quote_diff = new_quote
            .checked_sub(v_quote)
            .ok_or(AeonianError::MathOverflow)?;
        let price = quote_diff
            .checked_mul(PRECISION)
            .ok_or(AeonianError::MathOverflow)?
            .checked_div(delta_base as u128)
            .ok_or(AeonianError::MathOverflow)?;
        Ok(price)
    } else {
        let new_base = v_base
            .checked_add(delta_base as u128)
            .ok_or(AeonianError::MathOverflow)?;
        let new_quote = invariant
            .checked_div(new_base)
            .ok_or(AeonianError::MathOverflow)?;
        let quote_diff = v_quote
            .checked_sub(new_quote)
            .ok_or(AeonianError::MathOverflow)?;
        let price = quote_diff
            .checked_mul(PRECISION)
            .ok_or(AeonianError::MathOverflow)?
            .checked_div(delta_base as u128)
            .ok_or(AeonianError::MathOverflow)?;
        Ok(price)
    }
}

pub fn parse_and_validate_pyth_price(
    account_info: &AccountInfo,
    current_slot: u64,
) -> Result<u128> {
    let data = account_info.try_borrow_data()?;
    require!(data.len() >= 208, AeonianError::InvalidOracleData);

    // Byte offsets for Pyth v2 structure
    let magic = u32::from_le_bytes(data[0..4].try_into().unwrap());
    require!(magic == 0xa1b2c3d4, AeonianError::InvalidOracleMagic);

    let publish_slot = u64::from_le_bytes(data[40..48].try_into().unwrap());
    let price = i64::from_le_bytes(data[208..216].try_into().unwrap());
    let conf = u64::from_le_bytes(data[216..224].try_into().unwrap());

    require!(price > 0, AeonianError::NegativeOraclePrice);
    require!(
        current_slot.saturating_sub(publish_slot) <= MAX_STALENESS_SLOTS,
        AeonianError::StaleOraclePrice
    );

    let price_u128 = price as u128;
    let conf_ratio = (conf as u128)
        .checked_mul(1_000_000)
        .ok_or(AeonianError::MathOverflow)?
        .checked_div(price_u128)
        .ok_or(AeonianError::MathOverflow)?;

    require!(
        conf_ratio <= MAX_CONF_RATIO as u128,
        AeonianError::ConfidenceIntervalTooWide
    );

    Ok(price_u128)
}

// -----------------------------------------------------------------------------
// ACCOUNT CONTEXTS & DATA STRUCTS
// -----------------------------------------------------------------------------
#[derive(Accounts)]
#[instruction(market_id: u64)]
pub struct InitializeMarket<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    #[account(
        init,
        payer = admin,
        space = 8 + Market::LEN,
        seeds = [b"market", market_id.to_le_bytes().as_ref()],
        bump
    )]
    pub market: Account<'info, Market>,

    pub collateral_mint: Account<'info, Mint>,

    /// CHECK: PDA signer for SPL vault authority
    #[account(
        seeds = [b"authority", market_id.to_le_bytes().as_ref()],
        bump
    )]
    pub vault_authority: UncheckedAccount<'info>,

    #[account(
        init,
        payer = admin,
        token::mint = collateral_mint,
        token::authority = vault_authority,
        seeds = [b"vault", market_id.to_le_bytes().as_ref()],
        bump
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

    #[account(
        mut,
        seeds = [b"market", market.market_id.to_le_bytes().as_ref()],
        bump = market.bump
    )]
    pub market: Account<'info, Market>,

    #[account(
        init_if_needed,
        payer = user,
        space = 8 + std::mem::size_of::<UserMarginData>(),
        seeds = [b"margin", user.key().as_ref(), market.key().as_ref()],
        bump
    )]
    pub user_margin: AccountLoader<'info, UserMarginData>,

    #[account(
        mut,
        constraint = user_token_account.owner == user.key() @ AeonianError::InvalidOwner
    )]
    pub user_token_account: Account<'info, TokenAccount>,

    #[account(
        mut,
        seeds = [b"vault", market.market_id.to_le_bytes().as_ref()],
        bump
    )]
    pub vault: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct OpenPosition<'info> {
    pub user: Signer<'info>,

    #[account(
        mut,
        seeds = [b"market", market.market_id.to_le_bytes().as_ref()],
        bump = market.bump
    )]
    pub market: Account<'info, Market>,

    #[account(
        mut,
        seeds = [b"margin", user.key().as_ref(), market.key().as_ref()],
        bump
    )]
    pub user_margin: AccountLoader<'info, UserMarginData>,

    /// CHECK: Validated inside parse_and_validate_pyth_price
    pub pyth_oracle_price: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct CrankLiquidation<'info> {
    #[account(mut)]
    pub liquidator: Signer<'info>,

    #[account(
        mut,
        seeds = [b"market", market.market_id.to_le_bytes().as_ref()],
        bump = market.bump
    )]
    pub market: Account<'info, Market>,

    #[account(
        mut,
        seeds = [b"margin", user_margin.load()?.owner.as_ref(), market.key().as_ref()],
        bump
    )]
    pub user_margin: AccountLoader<'info, UserMarginData>,

    /// CHECK: PDA signer authority
    #[account(
        seeds = [b"authority", market.market_id.to_le_bytes().as_ref()],
        bump = market.vault_bump
    )]
    pub vault_authority: UncheckedAccount<'info>,

    #[account(
        mut,
        seeds = [b"vault", market.market_id.to_le_bytes().as_ref()],
        bump
    )]
    pub vault: Account<'info, TokenAccount>,

    #[account(mut)]
    pub liquidator_token_account: Account<'info, TokenAccount>,

    /// CHECK: Pyth price validation
    pub pyth_oracle_price: UncheckedAccount<'info>,

    pub token_program: Program<'info, Token>,
}

// -----------------------------------------------------------------------------
// STATE STRUCTS (ZERO COPY OPTIMIZED)
// -----------------------------------------------------------------------------
#[account]
pub struct Market {
    pub admin: Pubkey,
    pub market_id: u64,
    pub collateral_mint: Pubkey,
    pub vault: Pubkey,
    pub virtual_quote_reserves: u128,
    pub virtual_base_reserves: u128,
    pub total_long_open_interest: u64,
    pub total_short_open_interest: u64,
    pub maintenance_margin_bps: u16,
    pub last_oracle_slot: u64,
    pub bump: u8,
    pub vault_bump: u8,
}

impl Market {
    pub const LEN: usize = 32 + 8 + 32 + 32 + 16 + 16 + 8 + 8 + 2 + 8 + 1 + 1 + 64;
}

#[account(zero_copy)]
#[derive(Default)]
#[repr(C)]
pub struct UserMarginData {
    pub owner: Pubkey,               // 32
    pub market: Pubkey,              // 32
    pub collateral_amount: u64,      // 8
    pub position_size: u64,          // 8
    pub entry_price: u64,            // 8
    pub last_interaction_slot: u64,  // 8
    pub is_long: u8,                 // 1
    pub _reserved: [u8; 31],         // 31 byte alignment padding
}

// -----------------------------------------------------------------------------
// EVENTS & ERRORS
// -----------------------------------------------------------------------------
#[event]
pub struct MarketInitializedEvent {
    pub market_id: u64,
    pub maintenance_margin_bps: u16,
    pub timestamp: i64,
}

#[event]
pub struct CollateralDepositedEvent {
    pub user: Pubkey,
    pub amount: u64,
    pub slot: u64,
}

#[event]
pub struct PositionOpenedEvent {
    pub user: Pubkey,
    pub market: Pubkey,
    pub size: u64,
    pub execution_price: u64,
    pub is_long: bool,
    pub slot: u64,
}

#[event]
pub struct PositionLiquidatedEvent {
    pub user: Pubkey,
    pub liquidator: Pubkey,
    pub reward: u64,
    pub slot: u64,
}

#[error_code]
pub enum AeonianError {
    #[msg("Invalid input amount")]
    InvalidAmount,
    #[msg("Calculation overflow")]
    MathOverflow,
    #[msg("Invalid owner provided")]
    InvalidOwner,
    #[msg("Position size must be greater than zero")]
    InvalidPositionSize,
    #[msg("Single-slot frontrun protection triggered")]
    SingleSlotExecutionLocked,
    #[msg("Execution slippage exceeds defined limit")]
    SlippageExceeded,
    #[msg("Insufficient pool reserves")]
    InsufficientReserves,
    #[msg("Insufficient collateral to maintain leverage")]
    InsufficientCollateral,
    #[msg("No active position for this margin account")]
    NoActivePosition,
    #[msg("Position is healthy and cannot be liquidated")]
    PositionHealthy,
    #[msg("Invalid Pyth oracle buffer layout")]
    InvalidOracleData,
    #[msg("Invalid oracle magic header")]
    InvalidOracleMagic,
    #[msg("Oracle price feed is negative or zero")]
    NegativeOraclePrice,
    #[msg("Oracle price feed is stale")]
    StaleOraclePrice,
    #[msg("Pyth confidence interval exceeds safety threshold")]
    ConfidenceIntervalTooWide,
    #[msg("Pool reserves cannot be zero")]
    ZeroReserves,
}