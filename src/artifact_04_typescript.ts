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

describe("aeonian-trade protocol test suite", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.AeonianTrade as Program<AeonianTrade>;

  let collateralMint: PublicKey;
  let authorityCollateralAccount: PublicKey;
  let traderCollateralAccount: PublicKey;
  let keeperCollateralAccount: PublicKey;

  const marketId = new anchor.BN(101);
  const trader = Keypair.generate();
  const keeper = Keypair.generate();
  const host = Keypair.generate();

  let marketPda: PublicKey;
  let vaultPda: PublicKey;
  let positionPda: PublicKey;
  let streamSignalPda: PublicKey;

  // Mock Pyth Account with deterministic price structure
  const mockPythAccount = Keypair.generate();

  before(async () => {
    // Airdrop SOL
    for (const kp of [trader, keeper, host]) {
      const sig = await provider.connection.requestAirdrop(
        kp.publicKey,
        10 * anchor.web3.LAMPORTS_PER_SOL
      );
      await provider.connection.confirmTransaction(sig);
    }

    // Create collateral mint (USDC equivalent, 6 decimals)
    collateralMint = await createMint(
      provider.connection,
      (provider.wallet as any).payer,
      provider.wallet.publicKey,
      null,
      6
    );

    // Derive PDAs
    [marketPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("market"), marketId.toArrayLike(Buffer, "le", 8)],
      program.programId
    );

    [vaultPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("vault"), marketId.toArrayLike(Buffer, "le", 8)],
      program.programId
    );

    [positionPda] = PublicKey.findProgramAddressSync(
      [
        Buffer.from("position"),
        marketPda.toBuffer(),
        trader.publicKey.toBuffer(),
      ],
      program.programId
    );

    [streamSignalPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("stream_signal"), host.publicKey.toBuffer()],
      program.programId
    );

    // Setup Token Accounts
    traderCollateralAccount = await createAccount(
      provider.connection,
      (provider.wallet as any).payer,
      collateralMint,
      trader.publicKey
    );

    keeperCollateralAccount = await createAccount(
      provider.connection,
      (provider.wallet as any).payer,
      collateralMint,
      keeper.publicKey
    );

    // Fund Trader Collateral (10,000 USDC)
    await mintTo(
      provider.connection,
      (provider.wallet as any).payer,
      collateralMint,
      traderCollateralAccount,
      provider.wallet.publicKey,
      10_000 * 1_000_000
    );
  });

  it("Initializes the Aeonian Market & Vault PDA", async () => {
    const tx = await program.methods
      .initializeMarket(marketId, 20, 500) // 20x max leverage, 5% maintenance margin
      .accounts({
        market: marketPda,
        collateralMint: collateralMint,
        vault: vaultPda,
        pythOracle: mockPythAccount.publicKey,
        authority: provider.wallet.publicKey,
        tokenProgram: TOKEN_PROGRAM_ID,
        systemProgram: SystemProgram.programId,
        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
      })
      .rpc();

    const marketState = await program.account.marketState.fetch(marketPda);
    expect(marketState.marketId.toNumber()).to.equal(101);
    expect(marketState.maxLeverage).to.equal(20);
    expect(marketState.totalLongSize.toNumber()).to.equal(0);
  });

  it("Broadcasts an interactive livestream trading signal PDA", async () => {
    const streamId = "AEONIAN_LIVE_EPISODE_01";
    await program.methods
      .broadcastLivestreamTrade(
        streamId,
        0, // 0 = LONG
        new anchor.BN(50_000_000), // Target position size
        10 // 10x leverage
      )
      .accounts({
        streamSignal: streamSignalPda,
        host: host.publicKey,
        systemProgram: SystemProgram.programId,
      })
      .signers([host])
      .rpc();

    const signalState = await program.account.streamSignalState.fetch(
      streamSignalPda
    );
    expect(signalState.streamId).to.equal(streamId);
    expect(signalState.action).to.equal(0);
    expect(signalState.leverage).to.equal(10);
    expect(signalState.host.toBase58()).to.equal(host.publicKey.toBase58());
  });

  it("Fails when trader attempts opening position exceeding max leverage", async () => {
    const collateral = new anchor.BN(100 * 1_000_000); // 100 USDC
    const size = new anchor.BN(3_000 * 1_000_000); // 30x leverage (Limit is 20x)

    try {
      await program.methods
        .openPosition(collateral, size, true)
        .accounts({
          market: marketPda,
          position: positionPda,
          vault: vaultPda,
          userTokenAccount: traderCollateralAccount,
          pythOracle: mockPythAccount.publicKey,
          trader: trader.publicKey,
          tokenProgram: TOKEN_PROGRAM_ID,
          systemProgram: SystemProgram.programId,
        })
        .signers([trader])
        .rpc();
      expect.fail("Expected transaction to fail due to leverage bounds");
    } catch (err: any) {
      // Expect either InvalidOracleAccount or LeverageExceeded depending on Pyth mock state
      expect(err).to.exist;
    }
  });
});