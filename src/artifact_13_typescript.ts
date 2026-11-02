import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { AeonianTrade } from "../target/types/aeonian_trade";
import {
  createMint,
  createAccount,
  mintTo,
  TOKEN_PROGRAM_ID,
} from "@solana/spl-token";
import { Keypair, PublicKey, SystemProgram } from "@solana/web3.js";
import { expect } from "chai";

describe("aeonian_trade core test suite", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.AeonianTrade as Program<AeonianTrade>;

  let collateralMint: PublicKey;
  let adminTokenAccount: PublicKey;
  let userTokenAccount: PublicKey;
  let liquidatorTokenAccount: PublicKey;
  let insuranceVault: PublicKey;

  const admin = (provider.wallet as anchor.Wallet).payer;
  const user = Keypair.generate();
  const liquidator = Keypair.generate();
  const fakePythFeed = Keypair.generate();

  const marketId = new anchor.BN(101);
  let marketPda: PublicKey;
  let vaultPda: PublicKey;
  let userMarginPda: PublicKey;
  let positionPda: PublicKey;

  before(async () => {
    // Fund test wallets
    for (const kp of [user, liquidator]) {
      const tx = await provider.connection.requestAirdrop(kp.publicKey, 10 * anchor.web3.LAMPORTS_PER_SOL);
      const latest = await provider.connection.getLatestBlockhash();
      await provider.connection.confirmTransaction({
        signature: tx,
        blockhash: latest.blockhash,
        lastValidBlockHeight: latest.lastValidBlockHeight,
      });
    }

    // Deploy Collateral Token Mint (USDC simulation)
    collateralMint = await createMint(
      provider.connection,
      admin,
      admin.publicKey,
      null,
      6
    );

    adminTokenAccount = await createAccount(provider.connection, admin, collateralMint, admin.publicKey);
    userTokenAccount = await createAccount(provider.connection, user, collateralMint, user.publicKey);
    liquidatorTokenAccount = await createAccount(provider.connection, liquidator, collateralMint, liquidator.publicKey);
    insuranceVault = await createAccount(provider.connection, admin, collateralMint, admin.publicKey);

    await mintTo(provider.connection, admin, collateralMint, userTokenAccount, admin, 100_000 * 1e6);

    // Derive PDAs
    [marketPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("market"), marketId.toArrayLike(Buffer, "le", 8)],
      program.programId
    );

    [vaultPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("vault"), marketId.toArrayLike(Buffer, "le", 8)],
      program.programId
    );

    [userMarginPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("margin"), marketPda.toBuffer(), user.publicKey.toBuffer()],
      program.programId
    );

    [positionPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("position"), marketPda.toBuffer(), user.publicKey.toBuffer()],
      program.programId
    );
  });

  it("1. Initializes the Aeonian Market PDA & Vault", async () => {
    const vQuote = new anchor.BN(10_000_000).mul(new anchor.BN(1e6));
    const vBase = new anchor.BN(100_000).mul(new anchor.BN(1e6));

    await program.methods
      .initializeMarket(
        marketId,
        new anchor.BN(500), // 5.00% maintenance margin
        new anchor.BN(250), // 2.50% liquidator fee
        vQuote,
        vBase
      )
      .accounts({
        admin: admin.publicKey,
        collateralMint,
        market: marketPda,
        vault: vaultPda,
        systemProgram: SystemProgram.programId,
        tokenProgram: TOKEN_PROGRAM_ID,
        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
      })
      .rpc();

    const market = await program.account.marketState.fetch(marketPda);
    expect(market.marketId.toString()).to.equal(marketId.toString());
    expect(market.vQuoteReserve.toString()).to.equal(vQuote.toString());
  });

  it("2. Deposits Collateral into Isolated Margin Vault", async () => {
    const depositAmount = new anchor.BN(5_000 * 1e6); // 5,000 USDC

    await program.methods
      .depositCollateral(depositAmount)
      .accounts({
        user: user.publicKey,
        market: marketPda,
        userMargin: userMarginPda,
        userTokenAccount: userTokenAccount,
        vault: vaultPda,
        tokenProgram: TOKEN_PROGRAM_ID,
        systemProgram: SystemProgram.programId,
      })
      .signers([user])
      .rpc();

    const marginAccount = await program.account.userMarginAccount.fetch(userMarginPda);
    expect(marginAccount.depositedCollateral.toNumber()).to.equal(5_000 * 1e6);
  });

  it("3. Rejects OpenPosition with Stale Oracle Payload", async () => {
    // Initialize mock pyth feed with expired slot
    const mockFeedData = Buffer.alloc(256);
    // Write stale slot at offset 112
    mockFeedData.writeBigUInt64LE(BigInt(10), 112);
    // Write Price $150 at offset 208
    mockFeedData.writeBigInt64LE(BigInt(150 * 1e6), 208);
    // Write tight confidence ratio at offset 216
    mockFeedData.writeBigUInt64LE(BigInt(1_000), 216);

    const rentExempt = await provider.connection.getMinimumBalanceForRentExemption(mockFeedData.length);
    const tx = new anchor.web3.Transaction().add(
      SystemProgram.createAccount({
        fromPubkey: admin.publicKey,
        newAccountPubkey: fakePythFeed.publicKey,
        lamports: rentExempt,
        space: mockFeedData.length,
        programId: SystemProgram.programId,
      })
    );
    await provider.sendAndConfirm(tx, [fakePythFeed]);

    let failed = false;
    try {
      await program.methods
        .openPosition(true, new anchor.BN(1000 * 1e6), new anchor.BN(0))
        .accounts({
          user: user.publicKey,
          market: marketPda,
          userMargin: userMarginPda,
          position: positionPda,
          pythOracleFeed: fakePythFeed.publicKey,
          systemProgram: SystemProgram.programId,
        })
        .signers([user])
        .rpc();
    } catch (e: any) {
      failed = true;
      expect(e.toString()).to.include("StaleOraclePrice");
    }
    expect(failed).to.be.true;
  });
});