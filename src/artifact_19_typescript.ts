import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { AeonianTrade } from "../target/types/aeonian_trade";
import {
  createMint,
  createAccount,
  mintTo,
  TOKEN_PROGRAM_ID,
} from "@solana/spl-token";
import { PublicKey, Keypair, SystemProgram } from "@solana/web3.js";
import { expect } from "chai";

describe("Aeonian Trade Full Protocol Lifecycle QA Matrix", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.AeonianTrade as Program<AeonianTrade>;

  let usdcMint: PublicKey;
  let admin = Keypair.generate();
  let trader = Keypair.generate();
  let keeper = Keypair.generate();

  let traderTokenAccount: PublicKey;
  let keeperTokenAccount: PublicKey;
  let marketPda: PublicKey;
  let vaultPda: PublicKey;
  let positionPda: PublicKey;

  const INITIAL_PRICE = new anchor.BN(150_000_000); // $150.000000
  const COLLATERAL_DEPOSIT = new anchor.BN(1_000_000_000); // 1,000 USDC

  before(async () => {
    // Fund test actors
    for (const actor of [admin, trader, keeper]) {
      const sig = await provider.connection.requestAirdrop(
        actor.publicKey,
        10 * anchor.web3.LAMPORTS_PER_SOL
      );
      await provider.connection.confirmTransaction(sig);
    }

    // Create Synthetic USDC Mint
    usdcMint = await createMint(
      provider.connection,
      admin,
      admin.publicKey,
      null,
      6
    );

    // Derive Market PDA
    [marketPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("market"), usdcMint.toBuffer()],
      program.programId
    );

    // Derive Vault PDA
    [vaultPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("vault"), marketPda.toBuffer(), usdcMint.toBuffer()],
      program.programId
    );

    // Derive Trader Position PDA
    [positionPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("position"), trader.publicKey.toBuffer(), marketPda.toBuffer()],
      program.programId
    );

    // Create Associated Token Accounts
    traderTokenAccount = await createAccount(
      provider.connection,
      trader,
      usdcMint,
      trader.publicKey
    );

    keeperTokenAccount = await createAccount(
      provider.connection,
      keeper,
      usdcMint,
      keeper.publicKey
    );

    // Mint USDC into Trader account
    await mintTo(
      provider.connection,
      admin,
      usdcMint,
      traderTokenAccount,
      admin.publicKey,
      5_000_000_000
    );
  });

  it("1. Initializes Market and Isolated Collateral Vault successfully", async () => {
    await program.methods
      .initializeMarket(INITIAL_PRICE, 20)
      .accounts({
        admin: admin.publicKey,
        assetMint: usdcMint,
        market: marketPda,
        vault: vaultPda,
        systemProgram: SystemProgram.programId,
        tokenProgram: TOKEN_PROGRAM_ID,
        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
      })
      .signers([admin])
      .rpc();

    const marketAccount = await program.account.market.fetch(marketPda);
    expect(marketAccount.markPrice.toNumber()).to.equal(INITIAL_PRICE.toNumber());
    expect(marketAccount.maxLeverage).to.equal(20);
    expect(marketAccount.openInterestLong.toNumber()).to.equal(0);
  });

  it("2. Fails to open position if Pyth oracle confidence interval is too wide", async () => {
    const invalidConfidence = new anchor.BN(5_000_000); // 3.3% confidence window > 0.20%
    try {
      await program.methods
        .openPosition(
          COLLATERAL_DEPOSIT,
          10,
          { long: {} },
          INITIAL_PRICE,
          invalidConfidence
        )
        .accounts({
          trader: trader.publicKey,
          market: marketPda,
          position: positionPda,
          traderTokenAccount: traderTokenAccount,
          vault: vaultPda,
          tokenProgram: TOKEN_PROGRAM_ID,
          systemProgram: SystemProgram.programId,
        })
        .signers([trader])
        .rpc();
      expect.fail("Expected transaction to fail due to high confidence spread");
    } catch (err: any) {
      expect(err.error.errorCode.code).to.equal("StaleOrUntrustedOracle");
    }
  });

  it("3. Successfully opens a 10x Long Position with valid parameters", async () => {
    const validConfidence = new anchor.BN(100_000); // 0.06% spread (well within bounds)

    const txSig = await program.methods
      .openPosition(
        COLLATERAL_DEPOSIT,
        10,
        { long: {} },
        INITIAL_PRICE,
        validConfidence
      )
      .accounts({
        trader: trader.publicKey,
        market: marketPda,
        position: positionPda,
        traderTokenAccount: traderTokenAccount,
        vault: vaultPda,
        tokenProgram: TOKEN_PROGRAM_ID,
        systemProgram: SystemProgram.programId,
      })
      .signers([trader])
      .rpc();

    const positionData = await program.account.position.fetch(positionPda);
    expect(positionData.collateral.toNumber()).to.equal(COLLATERAL_DEPOSIT.toNumber());
    expect(positionData.size.toNumber()).to.equal(COLLATERAL_DEPOSIT.toNumber() * 10);
    expect(positionData.entryPrice.toNumber()).to.equal(INITIAL_PRICE.toNumber());

    const marketData = await program.account.market.fetch(marketPda);
    expect(marketData.openInterestLong.toNumber()).to.equal(positionData.size.toNumber());
  });

  it("4. Prevents liquidation if position health factor is above threshold", async () => {
    // Current price is $145 (only a 3.3% drop, leverage 10x -> 33% loss vs 50% liquidation trigger)
    const healthyPrice = new anchor.BN(145_000_000);

    try {
      await program.methods
        .liquidatePosition(healthyPrice)
        .accounts({
          keeper: keeper.publicKey,
          market: marketPda,
          position: positionPda,
          vault: vaultPda,
          keeperTokenAccount: keeperTokenAccount,
          tokenProgram: TOKEN_PROGRAM_ID,
          systemProgram: SystemProgram.programId,
        })
        .signers([keeper])
        .rpc();
      expect.fail("Expected healthy position liquidation to revert");
    } catch (err: any) {
      expect(err.error.errorCode.code).to.equal("PositionHealthy");
    }
  });

  it("5. Allows keeper to liquidate when price drops beyond maintenance margin", async () => {
    // Price drops to $130 (13.33% drop * 10x leverage = 133% loss -> bankrupt / below MMR)
    const crashPrice = new anchor.BN(130_000_000);

    const tx = await program.methods
      .liquidatePosition(crashPrice)
      .accounts({
        keeper: keeper.publicKey,
        market: marketPda,
        position: positionPda,
        vault: vaultPda,
        keeperTokenAccount: keeperTokenAccount,
        tokenProgram: TOKEN_PROGRAM_ID,
        systemProgram: SystemProgram.programId,
      })
      .signers([keeper])
      .rpc();

    // Verify position account is closed
    const closedPosition = await program.account.position.fetchNullable(positionPda);
    expect(closedPosition).to.be.null;

    // Verify keeper received liquidation bounty
    const keeperBalance = await provider.connection.getTokenAccountBalance(keeperTokenAccount);
    expect(Number(keeperBalance.value.amount)).to.be.greaterThan(0);
  });
});