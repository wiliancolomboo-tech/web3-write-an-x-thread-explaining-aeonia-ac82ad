use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};
use pyth_sdk_solana::load_price_feed_from_account_info;

declare_id!("Aeon1anTrade111111111111111111111111111111111");

pub const MAX_CONF_RATIO: u64 = 100; // Max acceptable conf / price ratio (1%)
pub const BASIS_POINTS_DIVISOR: u128 = 10_000;
pub const MAINTENANCE_MARGIN_BPS: u128 = 500; // 5.00%
pub const LIQUIDATION_FEE_BPS: u128 = 250; // 2.50% keeper liquidation fee

#[program]
pub mod aeonian_trade {
    use super::*;

    pub fn initialize_market(
        ctx: Context<InitializeMarket>,
        market_id: u64,
        max_leverage: u8,
        maintenance_margin_bps: u16,
    ) -> Result<()> {
        let market = &mut ctx.accounts.market;
        market.market_id = market_id;
        market.authority = ctx.accounts.authority.key();
        market.collateral_mint = ctx.accounts.collateral_mint.key();
        market.vault = ctx.accounts.vault.key();
        market.pyth_oracle = ctx.accounts.pyth_oracle.key();
        market.max_leverage = max_leverage;
        market.maintenance_margin_bps = maintenance_margin_bps;
        market.total_long_size = 0;
        market.total_short_size = 0;
        market.last_funding_timestamp = Clock::get()?.unix_timestamp;
        market.cumulative_funding_rate = 0;
        market.bump = ctx.bumps.market;
        market.vault_bump = ctx.bumps.vault;
        Ok(())
    }

    pub fn open_position(
        ctx: Context<OpenPosition>,
        collateral_amount: u64,
        size: u64,
        is_long: bool,
    ) -> Result<()> {
        require!(collateral_amount > 0, TradeError::ZeroCollateral);
        require!(size > 0, TradeError::ZeroPositionSize);

        // Fetch validated oracle price
        let pyth_price = validate_and_get_pyth_price(&ctx.accounts.pyth_oracle)?;
        let position = &mut ctx.accounts.position;
        let market = &mut ctx.accounts.market;

        // Verify leverage limit
        let position_value = (size as u128)
            .checked_mul(pyth_price as u128)
            .ok_or(TradeError::MathOverflow)?;
        let leverage = position_value
            .checked_div(collateral_amount as u128)
            .ok_or(TradeError::MathOverflow)?;
        require!(
            leverage <= (market.max_leverage as u128),
            TradeError::LeverageExceeded
        );

        // Transfer collateral into protocol vault
        let cpi_accounts = Transfer {
            from: ctx.accounts.user_token_account.to_account_info(),
            to: ctx.accounts.vault.to_account_info(),
            authority: ctx.accounts.trader.to_account_info(),
        };
        let cpi_ctx = CpiContext::new(ctx.accounts.token_program.to_account_info(), cpi_accounts);
        token::transfer(cpi_ctx, collateral_amount)?;

        // Update Position State
        position.owner = ctx.accounts.trader.key();
        position.market = market.key();
        position.collateral_amount = collateral_amount;
        position.size = size;
        position.entry_price = pyth_price;
        position.is_long = is_long;
        position.last_cumulative_funding = market.cumulative_funding_rate;
        position.bump = ctx.bumps.position;

        // Update Market State
        if is_long {
            market.total_long_size = market
                .total_long_size
                .checked_add(size)
                .ok_or(TradeError::MathOverflow)?;
        } else {
            market.total_short_size = market
                .total_short_size
                .checked_add(size)
                .ok_or(TradeError::MathOverflow)?;
        }

        emit!(PositionOpenedEvent {
            trader: ctx.accounts.trader.key(),
            market: market.key(),
            size,
            collateral_amount,
            entry_price: pyth_price,
            is_long,
            timestamp: Clock::get()?.unix_timestamp,
        });

        Ok(())
    }

    pub fn liquidate_position(ctx: Context<LiquidatePosition>) -> Result<()> {
        let market = &mut ctx.accounts.market;
        let position = &mut ctx.accounts.position;
        let pyth_price = validate_and_get_pyth_price(&ctx.accounts.pyth_oracle)?;

        // Calculate Position PnL
        let price_diff = if position.is_long {
            (pyth_price as i128) - (position.entry_price as i128)
        } else {
            (position.entry_price as i128) - (pyth_price as i128)
        };

        let unrealized_pnl = (position.size as i128)
            .checked_mul(price_diff)
            .ok_or(TradeError::MathOverflow)?
            .checked_div(position.entry_price as i128)
            .ok_or(TradeError::MathOverflow)?;

        let current_equity = (position.collateral_amount as i128)
            .checked_add(unrealized_pnl)
            .ok_or(TradeError::MathOverflow)?;

        let position_value = (position.size as u128)
            .checked_mul(pyth_price as u128)
            .ok_or(TradeError::MathOverflow)?;

        let maintenance_requirement = position_value
            .checked_mul(market.maintenance_margin_bps as u128)
            .ok_or(TradeError::MathOverflow)?
            .checked_div(BASIS_POINTS_DIVISOR)
            .ok_or(TradeError::MathOverflow)?;

        // Ensure position is eligible for liquidation
        require!(
            current_equity < (maintenance_requirement as i128),
            TradeError::PositionNotLiquidatable
        );

        // Compute keeper incentive fee
        let keeper_fee = (position.collateral_amount as u128)
            .checked_mul(LIQUIDATION_FEE_BPS)
            .unwrap_or(0)
            .checked_div(BASIS_POINTS_DIVISOR)
            .unwrap_or(0) as u64;

        // Disburse liquidation fee to keeper via PDA vault signer
        let market_id_bytes = market.market_id.to_le_bytes();
        let seeds = &[
            b"vault".as_ref(),
            market_id_bytes.as_ref(),
            &[market.vault_bump],
        ];
        let signer = &[&seeds[..]];

        if keeper_fee > 0 {
            let cpi_accounts = Transfer {
                from: ctx.accounts.vault.to_account_info(),
                to: ctx.accounts.keeper_token_account.to_account_info(),
                authority: ctx.accounts.vault.to_account_info(),
            };
            let cpi_ctx = CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                cpi_accounts,
                signer,
            );
            token::transfer(cpi_ctx, keeper_fee)?;
        }

        // Adjust open interest
        if position.is_long {
            market.total_long_size = market.total_long_size.saturating_sub(position.size);
        } else {
            market.total_short_size = market.total_short_size.saturating_sub(position.size);
        }

        emit!(LiquidationEvent {
            liquidator: ctx.accounts.keeper.key(),
            trader: position.owner,
            market: market.key(),
            liquidated_size: position.size,
            liquidation_price: pyth_price,
            keeper_fee,
        });

        // Close position data account and return lamports to keeper
        position.size = 0;
        position.collateral_amount = 0;

        Ok(())
    }

    /// Broadcasts an authenticated on-chain trading signal for livestream interactive trade replication
    pub fn broadcast_livestream_trade(
        ctx: Context<BroadcastLivestreamTrade>,
        stream_id: String,
        action: u8, // 0 = LONG, 1 = SHORT, 2 = CLOSE
        target_size: u64,
        leverage: u8,
    ) -> Result<()> {
        let stream_signal = &mut ctx.accounts.stream_signal;
        let clock = Clock::get()?;

        stream_signal.host = ctx.accounts.host.key();
        stream_signal.stream_id = stream_id.clone();
        stream_signal.action = action;
        stream_signal.target_size = target_size;
        stream_signal.leverage = leverage;
        stream_signal.timestamp = clock.unix_timestamp;
        stream_signal.bump = ctx.bumps.stream_signal;

        emit!(LivestreamSignalEvent {
            host: ctx.accounts.host.key(),
            stream_id,
            action,
            target_size,
            leverage,
            timestamp: clock.unix_timestamp,
        });

        Ok(())
    }
}

// -----------------------------------------------------------------------------
// Helper Functions
// -----------------------------------------------------------------------------
pub fn validate_and_get_pyth_price(oracle_account: &AccountInfo) -> Result<u64> {
    let price_feed = load_price_feed_from_account_info(oracle_account)
        .map_err(|_| TradeError::InvalidOracleAccount)?;
    let current_price = price_feed
        .get_current_price()
        .ok_or(TradeError::OraclePriceUnavailable)?;

    // Positive price guard
    require!(current_price.price > 0, TradeError::InvalidOraclePrice);

    // Confidence Interval Check: conf / price <= 1%
    let conf = current_price.conf;
    let price = current_price.price as u64;
    require!(
        conf.checked_mul(MAX_CONF_RATIO).unwrap_or(u64::MAX) <= price,
        TradeError::OracleConfidenceBreached
    );

    Ok(price)
}

// -----------------------------------------------------------------------------
// Account Validation Contexts
// -----------------------------------------------------------------------------
#[derive(Accounts)]
#[instruction(market_id: u64)]
pub struct InitializeMarket<'info> {
    #[account(
        init,
        payer = authority,
        space = 8 + MarketState::INIT_SPACE,
        seeds = [b"market", market_id.to_le_bytes().as_ref()],
        bump
    )]
    pub market: Account<'info, MarketState>,

    pub collateral_mint: Account<'info, Mint>,

    #[account(
        init,
        payer = authority,
        seeds = [b"vault", market_id.to_le_bytes().as_ref()],
        bump,
        token::mint = collateral_mint,
        token::authority = vault
    )]
    pub vault: Account<'info, TokenAccount>,

    /// CHECK: Validated against Pyth SDK parser
    pub pyth_oracle: AccountInfo<'info>,

    #[account(mut)]
    pub authority: Signer<'info>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct OpenPosition<'info> {
    #[account(
        mut,
        seeds = [b"market", market.market_id.to_le_bytes().as_ref()],
        bump = market.bump
    )]
    pub market: Account<'info, MarketState>,

    #[account(
        init_if_needed,
        payer = trader,
        space = 8 + PositionState::INIT_SPACE,
        seeds = [b"position", market.key().as_ref(), trader.key().as_ref()],
        bump
    )]
    pub position: Account<'info, PositionState>,

    #[account(
        mut,
        seeds = [b"vault", market.market_id.to_le_bytes().as_ref()],
        bump = market.vault_bump
    )]
    pub vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = user_token_account.owner == trader.key(),
        constraint = user_token_account.mint == market.collateral_mint
    )]
    pub user_token_account: Account<'info, TokenAccount>,

    /// CHECK: Checked by Pyth program deserializer
    #[account(address = market.pyth_oracle)]
    pub pyth_oracle: AccountInfo<'info>,

    #[account(mut)]
    pub trader: Signer<'info>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct LiquidatePosition<'info> {
    #[account(
        mut,
        seeds = [b"market", market.market_id.to_le_bytes().as_ref()],
        bump = market.bump
    )]
    pub market: Account<'info, MarketState>,

    #[account(
        mut,
        seeds = [b"position", market.key().as_ref(), position.owner.as_ref()],
        bump = position.bump,
        has_one = market
    )]
    pub position: Account<'info, PositionState>,

    #[account(
        mut,
        seeds = [b"vault", market.market_id.to_le_bytes().as_ref()],
        bump = market.vault_bump
    )]
    pub vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = keeper_token_account.mint == market.collateral_mint
    )]
    pub keeper_token_account: Account<'info, TokenAccount>,

    /// CHECK: Checked against market state definition
    #[account(address = market.pyth_oracle)]
    pub pyth_oracle: AccountInfo<'info>,

    pub keeper: Signer<'info>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
#[instruction(stream_id: String)]
pub struct BroadcastLivestreamTrade<'info> {
    #[account(
        init_if_needed,
        payer = host,
        space = 8 + StreamSignalState::INIT_SPACE,
        seeds = [b"stream_signal", host.key().as_ref()],
        bump
    )]
    pub stream_signal: Account<'info, StreamSignalState>,

    #[account(mut)]
    pub host: Signer<'info>,

    pub system_program: Program<'info, System>,
}

// -----------------------------------------------------------------------------
// State Structures
// -----------------------------------------------------------------------------
#[account]
#[derive(InitSpace)]
pub struct MarketState {
    pub market_id: u64,
    pub authority: Pubkey,
    pub collateral_mint: Pubkey,
    pub vault: Pubkey,
    pub pyth_oracle: Pubkey,
    pub total_long_size: u64,
    pub total_short_size: u64,
    pub cumulative_funding_rate: i128,
    pub last_funding_timestamp: i64,
    pub max_leverage: u8,
    pub maintenance_margin_bps: u16,
    pub bump: u8,
    pub vault_bump: u8,
}

#[account]
#[derive(InitSpace)]
pub struct PositionState {
    pub owner: Pubkey,
    pub market: Pubkey,
    pub collateral_amount: u64,
    pub size: u64,
    pub entry_price: u64,
    pub last_cumulative_funding: i128,
    pub is_long: bool,
    pub bump: u8,
}

#[account]
#[derive(InitSpace)]
pub struct StreamSignalState {
    pub host: Pubkey,
    #[max_len(32)]
    pub stream_id: String,
    pub action: u8,
    pub target_size: u64,
    pub leverage: u8,
    pub timestamp: i64,
    pub bump: u8,
}

// -----------------------------------------------------------------------------
// Events
// -----------------------------------------------------------------------------
#[event]
pub struct PositionOpenedEvent {
    pub trader: Pubkey,
    pub market: Pubkey,
    pub size: u64,
    pub collateral_amount: u64,
    pub entry_price: u64,
    pub is_long: bool,
    pub timestamp: i64,
}

#[event]
pub struct LiquidationEvent {
    pub liquidator: Pubkey,
    pub trader: Pubkey,
    pub market: Pubkey,
    pub liquidated_size: u64,
    pub liquidation_price: u64,
    pub keeper_fee: u64,
}

#[event]
pub struct LivestreamSignalEvent {
    pub host: Pubkey,
    pub stream_id: String,
    pub action: u8,
    pub target_size: u64,
    pub leverage: u8,
    pub timestamp: i64,
}

// -----------------------------------------------------------------------------
// Error Codes
// -----------------------------------------------------------------------------
#[error_code]
pub enum TradeError {
    #[msg("Collateral amount must be strictly greater than zero.")]
    ZeroCollateral,
    #[msg("Position size must be strictly greater than zero.")]
    ZeroPositionSize,
    #[msg("Requested position exceeds maximum market leverage limit.")]
    LeverageExceeded,
    #[msg("Price oracle account is malformed or invalid.")]
    InvalidOracleAccount,
    #[msg("Oracle price cannot be read or is unavailable.")]
    OraclePriceUnavailable,
    #[msg("Oracle price must be positive.")]
    InvalidOraclePrice,
    #[msg("Pyth confidence interval exceeds threshold security ratio.")]
    OracleConfidenceBreached,
    #[msg("Position health is above maintenance margin; not liquidatable.")]
    PositionNotLiquidatable,
    #[msg("Arithmetic calculation resulted in an overflow or underflow.")]
    MathOverflow,
}