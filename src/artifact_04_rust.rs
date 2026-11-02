use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};

declare_id!("AeoN1anTradeEngine111111111111111111111111111");

pub const CONFIG_SEED: &[u8] = b"global-config";
pub const POSITION_SEED: &[u8] = b"position";
pub const VAULT_SEED: &[u8] = b"vault";
pub const MAX_ORACLE_STALENESS_SLOTS: u64 = 5;
pub const BASIS_POINTS_DIVISOR: u64 = 10_000;
pub const MAINTENANCE_MARGIN_BPS: u64 = 500; // 5% maintenance margin requirement (20x max leverage)
pub const LIQUIDATION_PENALTY_BPS: u64 = 250; // 2.5% incentive to liquidator

#[program]
pub mod aeonian_trade {
    use super::*;

    pub fn initialize_global_config(
        ctx: Context<InitializeGlobalConfig>,
        fee_recipient: Pubkey,
        protocol_fee_bps: u16,
    ) -> Result<()> {
        require!(protocol_fee_bps <= 500, CustomError::FeeTooHigh);
        let config = &mut ctx.accounts.global_config;
        config.authority = ctx.accounts.authority.key();
        config.fee_recipient = fee_recipient;
        config.protocol_fee_bps = protocol_fee_bps;
        config.is_locked = false;
        config.bump = ctx.bumps.global_config;
        Ok(())
    }

    pub fn open_position(
        ctx: Context<OpenPosition>,
        market_id: u64,
        collateral_amount: u64,
        leverage_x: u8,
        is_long: bool,
        pyth_reported_price: u64,
        pyth_publish_slot: u64,
    ) -> Result<()> {
        let clock = Clock::get()?;
        let config = &mut ctx.accounts.global_config;
        require!(!config.is_locked, CustomError::ProtocolLocked);

        // Security Guard: Pyth Oracle Staleness Check
        require!(
            clock.slot.saturating_sub(pyth_publish_slot) <= MAX_ORACLE_STALENESS_SLOTS,
            CustomError::StaleOraclePrice
        );
        require!(pyth_reported_price > 0, CustomError::InvalidOraclePrice);
        require!(leverage_x > 0 && leverage_x <= 50, CustomError::ExcessiveLeverage);
        require!(collateral_amount > 0, CustomError::ZeroCollateral);

        // Calculate size: size = collateral * leverage
        let size_usd = collateral_amount
            .checked_mul(leverage_x as u64)
            .ok_or(CustomError::MathOverflow)?;

        // Transfer collateral into protocol vault
        let cpi_accounts = Transfer {
            from: ctx.accounts.user_token_account.to_account_info(),
            to: ctx.accounts.collateral_vault.to_account_info(),
            authority: ctx.accounts.trader.to_account_info(),
        };
        let cpi_ctx = CpiContext::new(ctx.accounts.token_program.to_account_info(), cpi_accounts);
        token::transfer(cpi_ctx, collateral_amount)?;

        // Populate trader position PDA
        let position = &mut ctx.accounts.position;
        position.trader = ctx.accounts.trader.key();
        position.market_id = market_id;
        position.collateral_amount = collateral_amount;
        position.size_usd = size_usd;
        position.entry_price = pyth_reported_price;
        position.is_long = is_long;
        position.last_updated_slot = clock.slot;
        position.bump = ctx.bumps.position;

        emit!(PositionOpened {
            trader: position.trader,
            market_id,
            collateral_amount,
            size_usd,
            entry_price: pyth_reported_price,
            is_long,
            slot: clock.slot,
        });

        Ok(())
    }

    pub fn liquidate_position(
        ctx: Context<LiquidatePosition>,
        pyth_reported_price: u64,
        pyth_publish_slot: u64,
    ) -> Result<()> {
        let clock = Clock::get()?;
        require!(
            clock.slot.saturating_sub(pyth_publish_slot) <= MAX_ORACLE_STALENESS_SLOTS,
            CustomError::StaleOraclePrice
        );

        let position = &ctx.accounts.position;
        let pnl_negative = calculate_unrealized_loss(
            position.is_long,
            position.entry_price,
            pyth_reported_price,
            position.size_usd,
        )?;

        // Remaining Equity = Collateral - Loss
        let remaining_equity = position
            .collateral_amount
            .checked_sub(pnl_negative)
            .unwrap_or(0);

        // Maintenance Margin Required = Size * MAINTENANCE_MARGIN_BPS / 10,000
        let maintenance_margin = position
            .size_usd
            .checked_mul(MAINTENANCE_MARGIN_BPS)
            .ok_or(CustomError::MathOverflow)?
            .checked_div(BASIS_POINTS_DIVISOR)
            .ok_or(CustomError::MathOverflow)?;

        // Guard: Liquidate only if remaining_equity < maintenance_margin
        require!(
            remaining_equity < maintenance_margin,
            CustomError::PositionSolvent
        );

        // Calculate liquidation bounty for keeper
        let bounty_amount = position
            .collateral_amount
            .checked_mul(LIQUIDATION_PENALTY_BPS)
            .ok_or(CustomError::MathOverflow)?
            .checked_div(BASIS_POINTS_DIVISOR)
            .ok_or(CustomError::MathOverflow)?;

        let protocol_share = position
            .collateral_amount
            .saturating_sub(bounty_amount);

        // Transfer bounty to liquidator
        let vault_seeds: &[&[&[u8]]] = &[&[
            VAULT_SEED,
            ctx.accounts.vault_mint.to_account_info().key.as_ref(),
            &[ctx.bumps.collateral_vault],
        ]];

        let liquidator_cpi = Transfer {
            from: ctx.accounts.collateral_vault.to_account_info(),
            to: ctx.accounts.liquidator_token_account.to_account_info(),
            authority: ctx.accounts.collateral_vault.to_account_info(),
        };
        token::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                liquidator_cpi,
                vault_seeds,
            ),
            bounty_amount,
        )?;

        emit!(PositionLiquidated {
            trader: position.trader,
            liquidator: ctx.accounts.liquidator.key(),
            market_id: position.market_id,
            liquidated_collateral: position.collateral_amount,
            bounty_paid: bounty_amount,
            residual_to_vault: protocol_share,
            slot: clock.slot,
        });

        // Close position account and refund rent to trader
        ctx.accounts.position.close(ctx.accounts.trader.to_account_info())?;

        Ok(())
    }
}

// Math helper for unrealized loss calculation
fn calculate_unrealized_loss(
    is_long: bool,
    entry_price: u64,
    current_price: u64,
    size_usd: u64,
) -> Result<u64> {
    if is_long {
        if current_price < entry_price {
            let price_drop = entry_price.saturating_sub(current_price);
            let loss = (price_drop as u128)
                .checked_mul(size_usd as u128)
                .ok_or(CustomError::MathOverflow)?
                .checked_div(entry_price as u128)
                .ok_or(CustomError::MathOverflow)?;
            Ok(loss as u64)
        } else {
            Ok(0)
        }
    } else {
        if current_price > entry_price {
            let price_spike = current_price.saturating_sub(entry_price);
            let loss = (price_spike as u128)
                .checked_mul(size_usd as u128)
                .ok_or(CustomError::MathOverflow)?
                .checked_div(entry_price as u128)
                .ok_or(CustomError::MathOverflow)?;
            Ok(loss as u64)
        } else {
            Ok(0)
        }
    }
}

#[derive(Accounts)]
pub struct InitializeGlobalConfig<'info> {
    #[account(
        init,
        payer = authority,
        space = 8 + GlobalConfig::LEN,
        seeds = [CONFIG_SEED],
        bump
    )]
    pub global_config: Account<'info, GlobalConfig>,
    #[account(mut)]
    pub authority: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(market_id: u64)]
pub struct OpenPosition<'info> {
    #[account(
        init_if_needed,
        payer = trader,
        space = 8 + Position::LEN,
        seeds = [POSITION_SEED, trader.key().as_ref(), &market_id.to_le_bytes()],
        bump
    )]
    pub position: Account<'info, Position>,
    #[account(mut)]
    pub trader: Signer<'info>,
    #[account(
        mut,
        constraint = user_token_account.owner == trader.key(),
        constraint = user_token_account.mint == collateral_mint.key()
    )]
    pub user_token_account: Account<'info, TokenAccount>,
    pub collateral_mint: Account<'info, Mint>,
    #[account(
        mut,
        seeds = [VAULT_SEED, collateral_mint.key().as_ref()],
        bump
    )]
    pub collateral_vault: Account<'info, TokenAccount>,
    #[account(seeds = [CONFIG_SEED], bump = global_config.bump)]
    pub global_config: Account<'info, GlobalConfig>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct LiquidatePosition<'info> {
    #[account(
        mut,
        seeds = [POSITION_SEED, position.trader.as_ref(), &position.market_id.to_le_bytes()],
        bump = position.bump,
        has_one = trader
    )]
    pub position: Account<'info, Position>,
    /// CHECK: Safe because trader receives back rent exemption lamports upon account close
    #[account(mut)]
    pub trader: AccountInfo<'info>,
    #[account(mut)]
    pub liquidator: Signer<'info>,
    #[account(
        mut,
        constraint = liquidator_token_account.owner == liquidator.key(),
        constraint = liquidator_token_account.mint == vault_mint.key()
    )]
    pub liquidator_token_account: Account<'info, TokenAccount>,
    pub vault_mint: Account<'info, Mint>,
    #[account(
        mut,
        seeds = [VAULT_SEED, vault_mint.key().as_ref()],
        bump
    )]
    pub collateral_vault: Account<'info, TokenAccount>,
    pub token_program: Program<'info, Token>,
}

#[account]
pub struct GlobalConfig {
    pub authority: Pubkey,
    pub fee_recipient: Pubkey,
    pub protocol_fee_bps: u16,
    pub is_locked: bool,
    pub bump: u8,
}

impl GlobalConfig {
    pub const LEN: usize = 32 + 32 + 2 + 1 + 1;
}

#[account]
pub struct Position {
    pub trader: Pubkey,
    pub market_id: u64,
    pub collateral_amount: u64,
    pub size_usd: u64,
    pub entry_price: u64,
    pub is_long: bool,
    pub last_updated_slot: u64,
    pub bump: u8,
}

impl Position {
    pub const LEN: usize = 32 + 8 + 8 + 8 + 8 + 1 + 8 + 1;
}

#[event]
pub struct PositionOpened {
    pub trader: Pubkey,
    pub market_id: u64,
    pub collateral_amount: u64,
    pub size_usd: u64,
    pub entry_price: u64,
    pub is_long: bool,
    pub slot: u64,
}

#[event]
pub struct PositionLiquidated {
    pub trader: Pubkey,
    pub liquidator: Pubkey,
    pub market_id: u64,
    pub liquidated_collateral: u64,
    pub bounty_paid: u64,
    pub residual_to_vault: u64,
    pub slot: u64,
}

#[error_code]
pub enum CustomError {
    #[msg("Protocol administrative action is currently locked")]
    ProtocolLocked,
    #[msg("Protocol fee exceeds maximum allowed threshold")]
    FeeTooHigh,
    #[msg("Reported oracle price is stale or expired")]
    StaleOraclePrice,
    #[msg("Oracle price cannot be zero")]
    InvalidOraclePrice,
    #[msg("Requested leverage exceeds maximum threshold")]
    ExcessiveLeverage,
    #[msg("Collateral deposit must be greater than zero")]
    ZeroCollateral,
    #[msg("Arithmetic overflow or underflow detected")]
    MathOverflow,
    #[msg("Position health is above liquidation threshold")]
    PositionSolvent,
}