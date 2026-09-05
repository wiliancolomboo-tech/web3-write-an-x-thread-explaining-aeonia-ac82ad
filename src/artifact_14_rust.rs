use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer};

declare_id!("AeonTrade11111111111111111111111111111111111");

pub const MAX_POSITIONS: usize = 4;
pub const MAX_ORACLE_STALENESS_SLOTS: u64 = 3;
pub const BASIS_POINTS_DIVISOR: u128 = 10_000;
pub const MAINTENANCE_MARGIN_BPS: u128 = 500; // 5.00% minimum margin (20x max leverage)
pub const LIQUIDATION_PENALTY_BPS: u128 = 250; // 2.50% penalty

#[program]
pub mod aeonian_trade {
    use super::*;

    pub fn initialize_market(
        ctx: Context<InitializeMarket>,
        market_index: u16,
        oracle_pubkey: Pubkey,
        initial_funding_rate: i64,
    ) -> Result<()> {
        let market = &mut ctx.accounts.market;
        market.market_index = market_index;
        market.authority = ctx.accounts.authority.key();
        market.vault = ctx.accounts.vault.key();
        market.oracle = oracle_pubkey;
        market.total_long_size = 0;
        market.total_short_size = 0;
        market.cumulative_funding_rate = initial_funding_rate;
        market.last_funding_timestamp = Clock::get()?.unix_timestamp;
        market.bump = ctx.bumps.market;
        Ok(())
    }

    pub fn initialize_margin_account(ctx: Context<InitializeMarginAccount>) -> Result<()> {
        let margin_acc = &mut ctx.accounts.margin_account.load_init()?;
        margin_acc.owner = ctx.accounts.owner.key();
        margin_acc.total_collateral = 0;
        margin_acc.position_count = 0;
        margin_acc.bump = ctx.bumps.margin_account;
        margin_acc.padding = [0; 7];
        Ok(())
    }

    pub fn deposit_collateral(ctx: Context<DepositCollateral>, amount: u64) -> Result<()> {
        require!(amount > 0, AeonError::ZeroDepositAmount);

        // CPI to transfer SPL collateral to vault
        let cpi_accounts = Transfer {
            from: ctx.accounts.user_token_account.to_account_info(),
            to: ctx.accounts.vault.to_account_info(),
            authority: ctx.accounts.owner.to_account_info(),
        };
        let cpi_ctx = CpiContext::new(ctx.accounts.token_program.to_account_info(), cpi_accounts);
        token::transfer(cpi_ctx, amount)?;

        let mut margin_acc = ctx.accounts.margin_account.load_mut()?;
        margin_acc.total_collateral = margin_acc
            .total_collateral
            .checked_add(amount)
            .ok_or(AeonError::NumericalOverflow)?;

        emit!(CollateralDeposited {
            user: ctx.accounts.owner.key(),
            amount,
            total_collateral: margin_acc.total_collateral,
        });

        Ok(())
    }

    pub fn open_position(
        ctx: Context<OpenPosition>,
        size_delta: i64,
        max_acceptable_price: u64,
    ) -> Result<()> {
        require!(size_delta != 0, AeonError::ZeroPositionSize);

        // 1. Validate Oracle Price Freshness
        let current_slot = Clock::get()?.slot;
        let pyth_price = validate_oracle_account(
            &ctx.accounts.oracle.to_account_info(),
            current_slot,
            MAX_ORACLE_STALENESS_SLOTS,
        )?;

        // Slippage check
        if size_delta > 0 {
            require!(pyth_price <= max_acceptable_price, AeonError::SlippageExceeded);
        } else {
            require!(pyth_price >= max_acceptable_price, AeonError::SlippageExceeded);
        }

        let mut margin_acc = ctx.accounts.margin_account.load_mut()?;
        let market = &mut ctx.accounts.market;

        // 2. Locate or allocate position slot
        let mut target_index: Option<usize> = None;
        for i in 0..margin_acc.position_count as usize {
            if margin_acc.positions[i].market_index == market.market_index {
                target_index = Some(i);
                break;
            }
        }

        if target_index.is_none() {
            require!(
                (margin_acc.position_count as usize) < MAX_POSITIONS,
                AeonError::MaxPositionsExceeded
            );
            target_index = Some(margin_acc.position_count as usize);
            margin_acc.position_count += 1;
            margin_acc.positions[target_index.unwrap()] = Position::default();
            margin_acc.positions[target_index.unwrap()].market_index = market.market_index;
        }

        let idx = target_index.unwrap();
        let pos = &mut margin_acc.positions[idx];

        // 3. Compute Position Updates & Entry Price
        let new_size = pos
            .size
            .checked_add(size_delta)
            .ok_or(AeonError::NumericalOverflow)?;

        pos.size = new_size;
        pos.entry_price = pyth_price;
        pos.last_cumulative_funding = market.cumulative_funding_rate;

        // Update Global Open Interest
        if size_delta > 0 {
            market.total_long_size = market
                .total_long_size
                .checked_add(size_delta as u64)
                .ok_or(AeonError::NumericalOverflow)?;
        } else {
            market.total_short_size = market
                .total_short_size
                .checked_add(size_delta.unsigned_abs())
                .ok_or(AeonError::NumericalOverflow)?;
        }

        // 4. Assert Margin Health Invariant
        let is_healthy = evaluate_margin_health(&margin_acc, pyth_price)?;
        require!(is_healthy, AeonError::InsufficientFreeCollateral);

        emit!(PositionModified {
            owner: margin_acc.owner,
            market_index: market.market_index,
            size: pos.size,
            execution_price: pyth_price,
        });

        Ok(())
    }

    pub fn liquidate_position(ctx: Context<LiquidatePosition>) -> Result<()> {
        let current_slot = Clock::get()?.slot;
        let pyth_price = validate_oracle_account(
            &ctx.accounts.oracle.to_account_info(),
            current_slot,
            MAX_ORACLE_STALENESS_SLOTS,
        )?;

        let mut margin_acc = ctx.accounts.margin_account.load_mut()?;
        let market = &mut ctx.accounts.market;

        // Ensure user is below maintenance margin
        let is_healthy = evaluate_margin_health(&margin_acc, pyth_price)?;
        require!(!is_healthy, AeonError::AccountNotEligibleForLiquidation);

        // Find position for target market
        let pos_idx = margin_acc
            .positions
            .iter()
            .position(|p| p.market_index == market.market_index && p.size != 0)
            .ok_or(AeonError::PositionNotFound)?;

        let position_size = margin_acc.positions[pos_idx].size;
        let notional_value = (position_size.unsigned_abs() as u128)
            .checked_mul(pyth_price as u128)
            .ok_or(AeonError::NumericalOverflow)?
            / 1_000_000;

        let liquidation_fee = notional_value
            .checked_mul(LIQUIDATION_PENALTY_BPS)
            .ok_or(AeonError::NumericalOverflow)?
            / BASIS_POINTS_DIVISOR;

        // Zero out liquidated position
        margin_acc.positions[pos_idx].size = 0;
        margin_acc.positions[pos_idx].entry_price = 0;

        // Deduct penalty from remaining collateral
        margin_acc.total_collateral = margin_acc
            .total_collateral
            .saturating_sub(liquidation_fee as u64);

        // Reward keeper via CPI
        let seeds = &[
            b"market".as_ref(),
            &market.market_index.to_le_bytes(),
            &[market.bump],
        ];
        let signer = &[&seeds[..]];

        let cpi_accounts = Transfer {
            from: ctx.accounts.vault.to_account_info(),
            to: ctx.accounts.liquidator_token_account.to_account_info(),
            authority: market.to_account_info(),
        };
        let cpi_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            cpi_accounts,
            signer,
        );
        token::transfer(cpi_ctx, liquidation_fee as u64)?;

        emit!(PositionLiquidated {
            liquidated_user: margin_acc.owner,
            liquidator: ctx.accounts.liquidator.key(),
            market_index: market.market_index,
            liquidated_size: position_size,
            execution_price: pyth_price,
            fee_awarded: liquidation_fee as u64,
        });

        Ok(())
    }
}

// -----------------------------------------------------------------------------
// HELPER FUNCTIONS & MATH ENGINE
// -----------------------------------------------------------------------------

fn validate_oracle_account(
    oracle_info: &AccountInfo,
    current_slot: u64,
    max_staleness: u64,
) -> Result<u64> {
    let data = oracle_info.try_borrow_data()?;
    require!(data.len() >= 64, AeonError::InvalidOracleAccount);

    // Mock Pyth payload byte parsing for reference implementation:
    // Offset 0..8: Price (i64), Offset 8..16: Slot (u64), Offset 16..24: Conf (u64)
    let price_bytes: [u8; 8] = data[0..8].try_into().unwrap();
    let slot_bytes: [u8; 8] = data[8..16].try_into().unwrap();

    let price = i64::from_le_bytes(price_bytes);
    let publish_slot = u64::from_le_bytes(slot_bytes);

    require!(price > 0, AeonError::InvalidOraclePrice);
    require!(
        current_slot.saturating_sub(publish_slot) <= max_staleness,
        AeonError::OracleStale
    );

    Ok(price as u64)
}

fn evaluate_margin_health(margin_account: &MarginAccount, oracle_price: u64) -> Result<bool> {
    let mut total_notional: u128 = 0;

    for i in 0..margin_account.position_count as usize {
        let pos = &margin_account.positions[i];
        if pos.size != 0 {
            let notional = (pos.size.unsigned_abs() as u128)
                .checked_mul(oracle_price as u128)
                .ok_or(AeonError::NumericalOverflow)?
                / 1_000_000;
            total_notional = total_notional
                .checked_add(notional)
                .ok_or(AeonError::NumericalOverflow)?;
        }
    }

    if total_notional == 0 {
        return Ok(true);
    }

    let required_margin = total_notional
        .checked_mul(MAINTENANCE_MARGIN_BPS)
        .ok_or(AeonError::NumericalOverflow)?
        / BASIS_POINTS_DIVISOR;

    Ok((margin_account.total_collateral as u128) >= required_margin)
}

// -----------------------------------------------------------------------------
// ACCOUNT STRUCTS & ZERO-COPY DEFINITIONS
// -----------------------------------------------------------------------------

#[account]
#[derive(Default)]
pub struct Market {
    pub market_index: u16,
    pub bump: u8,
    pub authority: Pubkey,
    pub vault: Pubkey,
    pub oracle: Pubkey,
    pub total_long_size: u64,
    pub total_short_size: u64,
    pub cumulative_funding_rate: i64,
    pub last_funding_timestamp: i64,
}

#[account(zero_copy)]
#[repr(C)]
pub struct MarginAccount {
    pub owner: Pubkey,            // 32 bytes
    pub total_collateral: u64,     // 8 bytes
    pub position_count: u8,        // 1 byte
    pub bump: u8,                  // 1 byte
    pub padding: [u8; 6],          // 6 bytes to preserve 8-byte alignment
    pub positions: [Position; 4],  // 4 * 32 bytes = 128 bytes
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Default, bytemuck::Zeroable, bytemuck::Pod)]
#[repr(C)]
pub struct Position {
    pub market_index: u16,              // 2 bytes
    pub padding: [u8; 6],               // 6 bytes alignment
    pub size: i64,                      // 8 bytes (+ long, - short)
    pub entry_price: u64,               // 8 bytes
    pub last_cumulative_funding: i64,   // 8 bytes
}

// -----------------------------------------------------------------------------
// INSTRUCTION CONTEXTS
// -----------------------------------------------------------------------------

#[derive(Accounts)]
#[instruction(market_index: u16)]
pub struct InitializeMarket<'info> {
    #[account(
        init,
        payer = authority,
        space = 8 + std::mem::size_of::<Market>(),
        seeds = [b"market", market_index.to_le_bytes().as_ref()],
        bump
    )]
    pub market: Account<'info, Market>,
    #[account(
        init,
        payer = authority,
        token::mint = collateral_mint,
        token::authority = market,
        seeds = [b"vault", market_index.to_le_bytes().as_ref()],
        bump
    )]
    pub vault: Account<'info, TokenAccount>,
    pub collateral_mint: Account<'info, token::Mint>,
    #[account(mut)]
    pub authority: Signer<'info>,
    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct InitializeMarginAccount<'info> {
    #[account(
        init,
        payer = owner,
        space = 8 + std::mem::size_of::<MarginAccount>(),
        seeds = [b"margin", owner.key().as_ref()],
        bump
    )]
    pub margin_account: AccountLoader<'info, MarginAccount>,
    #[account(mut)]
    pub owner: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct DepositCollateral<'info> {
    #[account(
        mut,
        seeds = [b"margin", owner.key().as_ref()],
        bump
    )]
    pub margin_account: AccountLoader<'info, MarginAccount>,
    #[account(mut)]
    pub user_token_account: Account<'info, TokenAccount>,
    #[account(mut)]
    pub vault: Account<'info, TokenAccount>,
    pub owner: Signer<'info>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct OpenPosition<'info> {
    #[account(
        mut,
        seeds = [b"margin", owner.key().as_ref()],
        bump
    )]
    pub margin_account: AccountLoader<'info, MarginAccount>,
    #[account(mut)]
    pub market: Account<'info, Market>,
    /// CHECK: Validated inside `validate_oracle_account` helper with staleness checks
    pub oracle: AccountInfo<'info>,
    pub owner: Signer<'info>,
}

#[derive(Accounts)]
pub struct LiquidatePosition<'info> {
    #[account(mut)]
    pub margin_account: AccountLoader<'info, MarginAccount>,
    #[account(mut)]
    pub market: Account<'info, Market>,
    /// CHECK: Validated against oracle constraints
    pub oracle: AccountInfo<'info>,
    #[account(mut)]
    pub vault: Account<'info, TokenAccount>,
    #[account(mut)]
    pub liquidator_token_account: Account<'info, TokenAccount>,
    pub liquidator: Signer<'info>,
    pub token_program: Program<'info, Token>,
}

// -----------------------------------------------------------------------------
// EVENTS & ERRORS
// -----------------------------------------------------------------------------

#[event]
pub struct CollateralDeposited {
    pub user: Pubkey,
    pub amount: u64,
    pub total_collateral: u64,
}

#[event]
pub struct PositionModified {
    pub owner: Pubkey,
    pub market_index: u16,
    pub size: i64,
    pub execution_price: u64,
}

#[event]
pub struct PositionLiquidated {
    pub liquidated_user: Pubkey,
    pub liquidator: Pubkey,
    pub market_index: u16,
    pub liquidated_size: i64,
    pub execution_price: u64,
    pub fee_awarded: u64,
}

#[error_code]
pub enum AeonError {
    #[msg("Zero deposit amount not permitted")]
    ZeroDepositAmount,
    #[msg("Numerical calculation overflow")]
    NumericalOverflow,
    #[msg("Zero position size modification")]
    ZeroPositionSize,
    #[msg("Execution price slipped past tolerance")]
    SlippageExceeded,
    #[msg("Position limits exceeded for margin account")]
    MaxPositionsExceeded,
    #[msg("Margin collateral is insufficient to cover maintenance")]
    InsufficientFreeCollateral,
    #[msg("Account margin is healthy; liquidation denied")]
    AccountNotEligibleForLiquidation,
    #[msg("No active position found on specified market")]
    PositionNotFound,
    #[msg("Supplied oracle account format is invalid")]
    InvalidOracleAccount,
    #[msg("Pyth price reported non-positive")]
    InvalidOraclePrice,
    #[msg("Oracle slot staleness exceeded acceptable limits")]
    OracleStale,
}