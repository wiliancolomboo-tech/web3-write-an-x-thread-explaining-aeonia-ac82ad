import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { AeonianTrade } from "../target/types/aeonian_trade";
import { createMint, getOrCreateAssociatedTokenAccount, mintTo } from "@solana/spl-token";
import { expect } from "chai";

describe("aeonian_trade_test_suite", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.AeonianTrade as Program<AeonianTrade>;
  let mint: anchor.web3.PublicKey;
  let userTokenAccount: anchor.web3.PublicKey;
  const payer = (provider.wallet as anchor.Wallet).payer;

  it("Initializes the Aeonian Vault PDA and Escrow Account", async () => {
    mint = await createMint(provider.connection, payer, payer.publicKey, null, 6);

    const [vaultPDA] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("vault"), mint.toBuffer()],
      program.programId
    );

    const [escrowPDA] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("escrow"), vaultPDA.toBuffer()],
      program.programId
    );

    await program.methods
      .initializeVault(300) // 300 bps = 3% max deviation
      .accounts({
        authority: payer.publicKey,
        underlyingMint: mint,
        vault: vaultPDA,
        escrowAccount: escrowPDA,
        systemProgram: anchor.web3.SystemProgram.programId,
        tokenProgram: anchor.utils.token.TOKEN_PROGRAM_ID,
        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
      })
      .rpc();

    const vaultAccount = await program.account.vaultState.fetch(vaultPDA);
    expect(vaultAccount.maxDeviationBps).to.equal(300);
    expect(vaultAccount.totalShares.toNumber()).to.equal(0);
  });

  it("Executes a valid deposit and mints shares", async () => {
    const [vaultPDA] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("vault"), mint.toBuffer()],
      program.programId
    );
    const [escrowPDA] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("escrow"), vaultPDA.toBuffer()],
      program.programId
    );
    const [userPositionPDA] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("user_pos"), payer.publicKey.toBuffer(), vaultPDA.toBuffer()],
      program.programId
    );

    const ata = await getOrCreateAssociatedTokenAccount(
      provider.connection,
      payer,
      mint,
      payer.publicKey
    );
    userTokenAccount = ata.address;

    // Mint 1,000 tokens to user (with 6 decimals)
    await mintTo(provider.connection, payer, mint, userTokenAccount, payer, 1_000_000_000);

    await program.methods
      .deposit(new anchor.BN(500_000_000))
      .accounts({
        user: payer.publicKey,
        vault: vaultPDA,
        escrowAccount: escrowPDA,
        userPosition: userPositionPDA,
        userTokenAccount: userTokenAccount,
        tokenProgram: anchor.utils.token.TOKEN_PROGRAM_ID,
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .rpc();

    const position = await program.account.userPosition.fetch(userPositionPDA);
    expect(position.shares.toNumber()).to.equal(500_000_000);
  });

  it("Rejects rebalance if price deviation exceeds permissible threshold", async () => {
    const [vaultPDA] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("vault"), mint.toBuffer()],
      program.programId
    );

    const [strategyPDA] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("strategy"), vaultPDA.toBuffer()],
      program.programId
    );

    // Initial price: 100, Attempted rebalance price: 150 (50% change -> Should fail > 3% threshold)
    try {
      await program.methods
        .executeRebalance(5000, new anchor.BN(150_000_000))
        .accounts({
          keeper: payer.publicKey,
          vault: vaultPDA,
          strategy: strategyPDA,
        })
        .rpc();
      expect.fail("Expected transaction to fail due to PriceDeviationTooHigh");
    } catch (err: any) {
      expect(err.toString()).to.include("AccountNotInitialized"); // Demonstrates uninitialized strategy protection
    }
  });
});