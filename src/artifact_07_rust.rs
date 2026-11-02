use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};

declare_id!("AeoN1anTrade11111111111111111111111111111111");

#[program]
pub mod aeonian_trade {
    use super::*;

    pub fn initialize_vault(
        ctx: Context<InitializeVault>,
        max_deviation_bps: u16,
    ) -> Result<()> {
        let vault = &mut ctx.accounts.vault;
        vault.authority = ctx.accounts.authority.key();
        vault.underlying_mint = ctx.accounts.underlying_mint.key();
        vault.escrow_account = ctx.accounts.escrow_account.key();
        vault.total_shares = 0;
        vault.is_paused = false;
        vault.max_deviation_bps = max_deviation_bps;
        vault.bump = ctx.bumps.vault;

        emit!(VaultInitializedEvent {
            vault: vault.key(),
            mint: vault.underlying_mint,
            max_deviation_bps,
        });

        Ok(())
    }

    pub fn deposit(ctx: Context<Deposit>, amount: u64) -> Result<()> {
        require!(!ctx.accounts.vault.is_paused, AeonianError::VaultPaused);
        require!(amount > 0, AeonianError::InvalidAmount);

        let vault = &ctx.accounts.vault;
        let escrow = &ctx.accounts.escrow_account;
        
        let shares_to_mint = if vault.total_shares == 0 || escrow.amount == 0 {
            amount
        } else {
            (amount as u128)
                .checked_mul(vault.total_shares as u128)
                .ok_or(AeonianError::MathOverflow)?
                .checked_div(escrow.amount as u128)
                .ok_or(AeonianError::MathOverflow)? as u64
        };

        // Transfer funds into PDA-controlled escrow
        let cpi_accounts = Transfer {
            from: ctx.accounts.user_token_account.to_account_info(),
            to: ctx.accounts.escrow_account.to_account_info(),
            authority: ctx.accounts.user.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        token::transfer(CpiContext::new(cpi_program, cpi_accounts), amount)?;

        // Update ledger states
        let user_pos = &mut ctx.accounts.user_position;
        user_pos.shares = user_pos
            .shares
            .checked_add(shares_to_mint)
            .ok_or(AeonianError::MathOverflow)?;
        user_pos.last_deposit_slot = Clock::get()?.slot;

        let vault_mut = &mut ctx.accounts.vault;
        vault_mut.total_shares = vault_mut
            .total_shares
            .checked_add(shares_to_mint)
            .ok_or(AeonianError::MathOverflow)?;

        emit!(DepositEvent {
            user: ctx.accounts.user.key(),
            amount,
            shares_minted: shares_to_mint,
        });

        Ok(())
    }

    pub fn execute_rebalance(
        ctx: Context<ExecuteRebalance>,
        target_allocation_bps: u16,
        oracle_price: u64,
    ) -> Result<()> {
        let vault = &ctx.accounts.vault;
        require!(!vault.is_paused, AeonianError::VaultPaused);
        require!(target_allocation_bps <= 10_000, AeonianError::InvalidBasisPoints);

        // Security check: Stale slot & reentrancy guard
        let current_slot = Clock::get()?.slot;
        let strategy = &mut ctx.accounts.strategy;
        require!(
            current_slot.saturating_sub(strategy.last_rebalance_slot) >= 2,
            AeonianError::ReentrancyCooldown
        );

        // Mock deviation check: Delta between oracle price and target hedge price
        let price_diff = strategy
            .last_executed_price
            .abs_diff(oracle_price);
        
        let diff_bps = (price_diff as u128)
            .checked_mul(10_000)
            .ok_or(AeonianError::MathOverflow)?
            .checked_div(strategy.last_executed_price.max(1) as u128)
            .ok_or(AeonianError::MathOverflow)? as u16;

        require!(
            diff_bps <= vault.max_deviation_bps,
            AeonianError::PriceDeviationTooHigh
        );

        strategy.last_executed_price = oracle_price;
        strategy.last_rebalance_slot = current_slot;
        strategy.rebalance_count = strategy
            .rebalance_count
            .checked_add(1)
            .ok_or(AeonianError::MathOverflow)?;

        emit!(RebalanceExecutedEvent {
            vault: vault.key(),
            price: oracle_price,
            slot: current_slot,
        });

        Ok(())
    }
}

#[derive(Accounts)]
pub struct InitializeVault<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    pub underlying_mint: Account<'info, Mint>,

    #[account(
        init,
        payer = authority,
        space = 8 + VaultState::INIT_SPACE,
        seeds = [b"vault", underlying_mint.key().as_ref()],
        bump
    )]
    pub vault: Account<'info, VaultState>,

    #[account(
        init,
        payer = authority,
        token::mint = underlying_mint,
        token::authority = vault,
        seeds = [b"escrow", vault.key().as_ref()],
        bump
    )]
    pub escrow_account: Account<'info, TokenAccount>,

    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct Deposit<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(
        mut,
        seeds = [b"vault", vault.underlying_mint.as_ref()],
        bump = vault.bump
    )]
    pub vault: Account<'info, VaultState>,

    #[account(
        mut,
        seeds = [b"escrow", vault.key().as_ref()],
        bump
    )]
    pub escrow_account: Account<'info, TokenAccount>,

    #[account(
        init_if_needed,
        payer = user,
        space = 8 + UserPosition::INIT_SPACE,
        seeds = [b"user_pos", user.key().as_ref(), vault.key().as_ref()],
        bump
    )]
    pub user_position: Account<'info, UserPosition>,

    #[account(
        mut,
        constraint = user_token_account.owner == user.key() @ AeonianError::InvalidOwner,
        constraint = user_token_account.mint == vault.underlying_mint @ AeonianError::InvalidMint
    )]
    pub user_token_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct ExecuteRebalance<'info> {
    pub keeper: Signer<'info>,

    #[account(
        seeds = [b"vault", vault.underlying_mint.as_ref()],
        bump = vault.bump
    )]
    pub vault: Account<'info, VaultState>,

    #[account(
        mut,
        seeds = [b"strategy", vault.key().as_ref()],
        bump = strategy.bump
    )]
    pub strategy: Account<'info, StrategyState>,
}

#[account]
#[derive(InitSpace)]
pub struct VaultState {
    pub authority: Pubkey,
    pub underlying_mint: Pubkey,
    pub escrow_account: Pubkey,
    pub total_shares: u64,
    pub max_deviation_bps: u16,
    pub is_paused: bool,
    pub bump: u8,
}

#[account]
#[derive(InitSpace)]
pub struct UserPosition {
    pub shares: u64,
    pub last_deposit_slot: u64,
}

#[account]
#[derive(InitSpace)]
pub struct StrategyState {
    pub vault: Pubkey,
    pub last_executed_price: u64,
    pub last_rebalance_slot: u64,
    pub rebalance_count: u64,
    pub bump: u8,
}

#[error_code]
pub enum AeonianError {
    #[msg("Vault is paused")]
    VaultPaused,
    #[msg("Invalid deposit amount")]
    InvalidAmount,
    #[msg("Mathematical overflow")]
    MathOverflow,
    #[msg("Basis points parameter invalid (max 10000)")]
    InvalidBasisPoints,
    #[msg("Reentrancy cooldown active")]
    ReentrancyCooldown,
    #[msg("Price deviation exceeds permissible threshold")]
    PriceDeviationTooHigh,
    #[msg("Owner does not match expected signer")]
    InvalidOwner,
    #[msg("Token mint mismatch")]
    InvalidMint,
}

#[event]
pub struct VaultInitializedEvent {
    pub vault: Pubkey,
    pub mint: Pubkey,
    pub max_deviation_bps: u16,
}

#[event]
pub struct DepositEvent {
    pub user: Pubkey,
    pub amount: u64,
    pub shares_minted: u64,
}

#[event]
pub struct RebalanceExecutedEvent {
    pub vault: Pubkey,
    pub price: u64,
    pub slot: u64,
}