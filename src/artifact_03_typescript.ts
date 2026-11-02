import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { AeonianTrade } from "../target/types/aeonian_trade";
import {
  Keypair,
  PublicKey,
  SystemProgram,
  LAMPORTS_PER_SOL,
} from "@solana/web3.js";
import {
  TOKEN_PROGRAM_ID,
  createMint,
  createAccount,
  mintTo,
  getAccount,
} from "@solana/spl-token";
import { expect } from "chai";

describe("Aeonian Trade: Protocol Integration & Security Suite", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.AeonianTrade as Program<AeonianTrade>;

  const marketAuthority = Keypair.generate();
  const trader = Keypair.generate();
  const keeper = Keypair.generate();

  let usdcMint: PublicKey;
  let traderUsdcAccount: PublicKey;
  let keeperUsdcAccount: PublicKey;
  let vaultCustodyAccount: PublicKey;

  let marketPda: PublicKey;
  let vaultPda: PublicKey;
  let oracleMockAccount: Keypair;

  const MARKET_ID = 101; // SOL-USD synthetic
  const POSITION_ID = new anchor.BN(1);

  before(async () => {
    // Fund all actors
    for (const actor of [marketAuthority, trader, keeper]) {
      const sig = await provider.connection.requestAirdrop(
        actor.publicKey,
        5 * LAMPORTS_PER_SOL
      );
      await provider.connection.confirmTransaction(sig);
    }

    // Initialize Mint & Token Accounts
    usdcMint = await createMint(
      provider.connection,
      marketAuthority,
      marketAuthority.publicKey,
      null,
      6
    );

    traderUsdcAccount = await createAccount(
      provider.connection,
      trader,
      usdcMint,
      trader.publicKey
    );

    keeperUsdcAccount = await createAccount(
      provider.connection,
      keeper,
      usdcMint,
      keeper.publicKey
    );

    // Derive PDAs
    const marketIdBuffer = Buffer.alloc(2);
    marketIdBuffer.writeUInt16LE(MARKET_ID, 0);

    [marketPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("market-state"), marketIdBuffer],
      program.programId
    );

    [vaultPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("alp-vault"), marketIdBuffer],
      program.programId
    );

    vaultCustodyAccount = await createAccount(
      provider.connection,
      marketAuthority,
      usdcMint,
      vaultPda
    );

    // Mint 10,000 USDC to trader
    await mintTo(
      provider.connection,
      marketAuthority,
      usdcMint,
      traderUsdcAccount,
      marketAuthority.publicKey,
      10_000 * 1_000_000
    );

    // Mock Oracle account with 8-byte price = $150.00 (150_000_000)
    oracleMockAccount = Keypair.generate();
    const priceData = Buffer.alloc(32);
    priceData.writeBigUInt64LE(BigInt(150_000_000), 0);

    const createOracleIx = SystemProgram.createAccount({
      fromPubkey: provider.wallet.publicKey,
      newAccountPubkey: oracleMockAccount.publicKey,
      lamports: await provider.connection.getMinimumBalanceForRentExemption(32),
      space: 32,
      programId: program.programId,
    });

    const tx = new anchor.web3.Transaction().add(createOracleIx);
    await provider.sendAndConfirm(tx, [oracleMockAccount]);
  });

  it("1. Initializes the Aeonian Market PDA and Configuration", async () => {
    await program.methods
      .initializeMarket(MARKET_ID, new anchor.BN(5000), new anchor.BN(10))
      .accounts({
        authority: marketAuthority.publicKey,
        market: marketPda,
        oracleFeed: oracleMockAccount.publicKey,
        systemProgram: SystemProgram.programId,
      })
      .signers([marketAuthority])
      .rpc();

    const marketData = await program.account.marketState.fetch(marketPda);
    expect(marketData.marketId).to.equal(MARKET_ID);
    expect(marketData.totalLongOi.toNumber()).to.equal(0);
    expect(marketData.totalShortOi.toNumber()).to.equal(0);
  });

  it("2. Opens a 10x leveraged Long position against Oracle Price with valid PDA isolation", async () => {
    const collateral = new anchor.BN(1_000 * 1_000_000); // 1,000 USDC
    const leverage = new anchor.BN(10); // 10x
    const maxAcceptablePrice = new anchor.BN(155_000_000); // Slippage guard: $155

    const marketIdBuffer = Buffer.alloc(2);
    marketIdBuffer.writeUInt16LE(MARKET_ID, 0);

    const posIdBuffer = Buffer.alloc(8);
    posIdBuffer.writeBigUInt64LE(BigInt(POSITION_ID.toNumber()), 0);

    const [positionPda] = PublicKey.findProgramAddressSync(
      [
        Buffer.from("position"),
        trader.publicKey.toBuffer(),
        marketIdBuffer,
        posIdBuffer,
      ],
      program.programId
    );

    await program.methods
      .openPosition(
        POSITION_ID,
        { long: {} },
        collateral,
        leverage,
        maxAcceptablePrice
      )
      .accounts({
        trader: trader.publicKey,
        market: marketPda,
        position: positionPda,
        traderTokenAccount: traderUsdcAccount,
        vault: vaultPda,
        vaultCustodyAccount: vaultCustodyAccount,
        oracleFeed: oracleMockAccount.publicKey,
        tokenProgram: TOKEN_PROGRAM_ID,
        systemProgram: SystemProgram.programId,
      })
      .signers([trader])
      .rpc();

    const position = await program.account.position.fetch(positionPda);
    expect(position.trader.toBase58()).to.equal(trader.publicKey.toBase58());
    expect(position.sizeUsd.toNumber()).to.equal(10_000 * 1_000_000);
    expect(position.entryPrice.toNumber()).to.equal(150_000_000);

    const marketData = await program.account.marketState.fetch(marketPda);
    expect(marketData.totalLongOi.toNumber()).to.equal(10_000 * 1_000_000);
  });

  it("3. Rejects illegal liquidation when position is healthy", async () => {
    const marketIdBuffer = Buffer.alloc(2);
    marketIdBuffer.writeUInt16LE(MARKET_ID, 0);
    const posIdBuffer = Buffer.alloc(8);
    posIdBuffer.writeBigUInt64LE(BigInt(POSITION_ID.toNumber()), 0);

    const [positionPda] = PublicKey.findProgramAddressSync(
      [
        Buffer.from("position"),
        trader.publicKey.toBuffer(),
        marketIdBuffer,
        posIdBuffer,
      ],
      program.programId
    );

    try {
      await program.methods
        .liquidatePosition()
        .accounts({
          keeper: keeper.publicKey,
          market: marketPda,
          position: positionPda,
          vault: vaultPda,
          vaultCustodyAccount: vaultCustodyAccount,
          keeperTokenAccount: keeperUsdcAccount,
          oracleFeed: oracleMockAccount.publicKey,
          tokenProgram: TOKEN_PROGRAM_ID,
        })
        .signers([keeper])
        .rpc();
      expect.fail("Liquidation should fail when margin ratio is healthy.");
    } catch (err: any) {
      expect(err.message).to.include("PositionNotLiquidatable");
    }
  });

  it("4. Prevents frontrunning slippage violations", async () => {
    const collateral = new anchor.BN(500 * 1_000_000);
    const leverage = new anchor.BN(5);
    const strictPrice = new anchor.BN(140_000_000); // Current oracle is 150_000_000 -> Should fail for Long

    const posId2 = new anchor.BN(2);
    const marketIdBuffer = Buffer.alloc(2);
    marketIdBuffer.writeUInt16LE(MARKET_ID, 0);
    const posIdBuffer = Buffer.alloc(8);
    posIdBuffer.writeBigUInt64LE(BigInt(2), 0);

    const [positionPda2] = PublicKey.findProgramAddressSync(
      [
        Buffer.from("position"),
        trader.publicKey.toBuffer(),
        marketIdBuffer,
        posIdBuffer,
      ],
      program.programId
    );

    try {
      await program.methods
        .openPosition(
          posId2,
          { long: {} },
          collateral,
          leverage,
          strictPrice
        )
        .accounts({
          trader: trader.publicKey,
          market: marketPda,
          position: positionPda2,
          traderTokenAccount: traderUsdcAccount,
          vault: vaultPda,
          vaultCustodyAccount: vaultCustodyAccount,
          oracleFeed: oracleMockAccount.publicKey,
          tokenProgram: TOKEN_PROGRAM_ID,
          systemProgram: SystemProgram.programId,
        })
        .signers([trader])
        .rpc();
      expect.fail("Trade should have been aborted due to slippage tolerance violation.");
    } catch (err: any) {
      expect(err.message).to.include("SlippageLimitExceeded");
    }
  });
});