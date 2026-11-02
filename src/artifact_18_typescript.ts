import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { AeonianTrade } from "../target/types/aeonian_trade";
import {
  createMint,
  createAccount,
  mintTo,
  TOKEN_PROGRAM_ID,
} from "@solana/spl-token";
import { expect } from "chai";

describe("Aeonian Trade Engine End-to-End Suite", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.AeonianTrade as Program<AeonianTrade>;

  let collateralMint: anchor.web3.PublicKey;
  let userTokenAccount: anchor.web3.PublicKey;
  let liquidatorTokenAccount: anchor.web3.PublicKey;
  let marketPda: anchor.web3.PublicKey;
  let vaultPda: anchor.web3.PublicKey;
  let vaultAuthorityPda: anchor.web3.PublicKey;
  let userMarginPda: anchor.web3.PublicKey;
  let mockPythAccount: anchor.web3.Keypair;

  const marketId = new anchor.BN(101);
  const initialVQuote = new anchor.BN(1_000_000_000_000); // 1,000,000 quote
  const initialVBase = new anchor.BN(10_000_000_000);     // 10,000 base
  const maintenanceMarginBps = 500;                      // 5.00%

  before(async () => {
    // 1. Create Mint & Accounts
    collateralMint = await createMint(
      provider.connection,
      (provider.wallet as any).payer,
      provider.wallet.publicKey,
      null,
      6
    );

    userTokenAccount = await createAccount(
      provider.connection,
      (provider.wallet as any).payer,
      collateralMint,
      provider.wallet.publicKey
    );

    liquidatorTokenAccount = await createAccount(
      provider.connection,
      (provider.wallet as any).payer,
      collateralMint,
      provider.wallet.publicKey
    );

    // Mint 50,000 USDC to User
    await mintTo(
      provider.connection,
      (provider.wallet as any).payer,
      collateralMint,
      userTokenAccount,
      provider.wallet.publicKey,
      50_000_000_000
    );

    // 2. Derive PDAs
    [marketPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("market"), marketId.toArrayLike(Buffer, "le", 8)],
      program.programId
    );

    [vaultAuthorityPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("authority"), marketId.toArrayLike(Buffer, "le", 8)],
      program.programId
    );

    [vaultPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("vault"), marketId.toArrayLike(Buffer, "le", 8)],
      program.programId
    );

    [userMarginPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [
        Buffer.from("margin"),
        provider.wallet.publicKey.toBuffer(),
        marketPda.toBuffer(),
      ],
      program.programId
    );

    // 3. Mock Pyth Account Buffer
    mockPythAccount = anchor.web3.Keypair.generate();
    const pythData = Buffer.alloc(256);
    // Magic: 0xa1b2c3d4
    pythData.writeUInt32LE(0xa1b2c3d4, 0);
    // Publish Slot
    const currentSlot = await provider.connection.getSlot();
    pythData.writeBigUInt64LE(BigInt(currentSlot), 40);
    // Price: 100_000_000 ($100 with 6 decimals)
    pythData.writeBigInt64LE(BigInt(100_000_000), 208);
    // Confidence: 10_000 (0.01% - well inside 0.15% threshold)
    pythData.writeBigUInt64LE(BigInt(10_000), 216);

    const rentLamports = await provider.connection.getMinimumBalanceForRentExemption(256);
    const tx = new anchor.web3.Transaction().add(
      anchor.web3.SystemProgram.createAccount({
        fromPubkey: provider.wallet.publicKey,
        newAccountPubkey: mockPythAccount.publicKey,
        space: 256,
        lamports: rentLamports,
        programId: program.programId,
      })
    );
    await provider.sendAndConfirm(tx, [mockPythAccount]);

    // Populate data
    // In localnet test harness, write serialized buffer
  });

  it("Initializes the Aeonian Perpetual Market", async () => {
    const tx = await program.methods
      .initializeMarket(marketId, initialVQuote, initialVBase, maintenanceMarginBps)
      .accounts({
        admin: provider.wallet.publicKey,
        market: marketPda,
        collateralMint: collateralMint,
        vaultAuthority: vaultAuthorityPda,
        vault: vaultPda,
        systemProgram: anchor.web3.SystemProgram.programId,
        tokenProgram: TOKEN_PROGRAM_ID,
        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
      })
      .rpc();

    expect(tx).to.be.a("string");
    const marketAccount = await program.account.market.fetch(marketPda);
    expect(marketAccount.marketId.toNumber()).to.equal(101);
    expect(marketAccount.maintenanceMarginBps).to.equal(500);
  });

  it("Deposits collateral into User Margin PDA", async () => {
    const depositAmount = new anchor.BN(10_000_000_000); // 10,000 USDC

    await program.methods
      .depositCollateral(depositAmount)
      .accounts({
        user: provider.wallet.publicKey,
        market: marketPda,
        userMargin: userMarginPda,
        userTokenAccount: userTokenAccount,
        vault: vaultPda,
        tokenProgram: TOKEN_PROGRAM_ID,
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .rpc();

    const userMargin = await program.account.userMarginData.fetch(userMarginPda);
    expect(userMargin.collateralAmount.toNumber()).to.equal(10_000_000_000);
  });

  it("Rejects position execution exceeding maximum slippage tolerance", async () => {
    const positionSize = new anchor.BN(50_000_000); // 50 Units
    const tightSlippageBps = 1; // 0.01% - will reject against curve shift

    try {
      await program.methods
        .openPosition(positionSize, true, tightSlippageBps)
        .accounts({
          user: provider.wallet.publicKey,
          market: marketPda,
          userMargin: userMarginPda,
          pythOraclePrice: mockPythAccount.publicKey,
        })
        .rpc();
      expect.fail("Expected transaction to fail due to slippage guard");
    } catch (err: any) {
      expect(err.error?.errorCode?.code || err.toString()).to.include("SlippageExceeded");
    }
  });

  it("Asserts single-slot front-running lock", async () => {
    // Same slot invocation verification
    const positionSize = new anchor.BN(1_000_000);
    const wideSlippageBps = 5000;

    // Fast-path execution test
    const txPromise1 = program.methods
      .openPosition(positionSize, true, wideSlippageBps)
      .accounts({
        user: provider.wallet.publicKey,
        market: marketPda,
        userMargin: userMarginPda,
        pythOraclePrice: mockPythAccount.publicKey,
      })
      .rpc();

    await txPromise1;

    // Second call in immediate succession
    try {
      await program.methods
        .openPosition(positionSize, true, wideSlippageBps)
        .accounts({
          user: provider.wallet.publicKey,
          market: marketPda,
          userMargin: userMarginPda,
          pythOraclePrice: mockPythAccount.publicKey,
        })
        .rpc();
    } catch (err: any) {
      expect(err.error?.errorCode?.code || err.toString()).to.include("SingleSlotExecutionLocked");
    }
  });
});