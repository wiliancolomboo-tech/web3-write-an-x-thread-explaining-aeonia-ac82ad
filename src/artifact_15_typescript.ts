import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { AeonianTrade } from "../target/types/aeonian_trade";
import { 
  createMint, 
  createAccount, 
  mintTo, 
  TOKEN_PROGRAM_ID 
} from "@solana/spl-token";
import { Keypair, SystemProgram, PublicKey } from "@solana/web3.js";
import { expect } from "chai";

describe("aeonian-trade lifecycle & risk tests", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.AeonianTrade as Program<AeonianTrade>;

  let collateralMint: PublicKey;
  let userTokenAccount: PublicKey;
  let liquidatorTokenAccount: PublicKey;
  let vaultPda: PublicKey;
  let marketPda: PublicKey;
  let marginPda: PublicKey;

  const marketIndex = 1;
  const mockOracleKeypair = Keypair.generate();

  before(async () => {
    // 1. Create collateral mint (USDC equivalent, 6 decimals)
    collateralMint = await createMint(
      provider.connection,
      (provider.wallet as any).payer,
      provider.wallet.publicKey,
      null,
      6
    );

    // 2. Derive Market & Vault PDAs
    const marketIndexBuffer = Buffer.alloc(2);
    marketIndexBuffer.writeUInt16LE(marketIndex, 0);

    [marketPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("market"), marketIndexBuffer],
      program.programId
    );

    [vaultPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("vault"), marketIndexBuffer],
      program.programId
    );

    [marginPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("margin"), provider.wallet.publicKey.toBuffer()],
      program.programId
    );

    // 3. Setup Token Accounts
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

    await mintTo(
      provider.connection,
      (provider.wallet as any).payer,
      collateralMint,
      userTokenAccount,
      provider.wallet.publicKey,
      1_000_000_000 // 1,000 USDC
    );
  });

  it("Initializes the perpetual market", async () => {
    await program.methods
      .initializeMarket(marketIndex, mockOracleKeypair.publicKey, new anchor.BN(0))
      .accounts({
        market: marketPda,
        vault: vaultPda,
        collateralMint: collateralMint,
        authority: provider.wallet.publicKey,
        systemProgram: SystemProgram.programId,
        tokenProgram: TOKEN_PROGRAM_ID,
        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
      })
      .rpc();

    const marketState = await program.account.market.fetch(marketPda);
    expect(marketState.marketIndex).to.equal(marketIndex);
  });

  it("Initializes a user zero-copy margin account", async () => {
    await program.methods
      .initializeMarginAccount()
      .accounts({
        marginAccount: marginPda,
        owner: provider.wallet.publicKey,
        systemProgram: SystemProgram.programId,
      })
      .rpc();

    const marginData = await program.account.marginAccount.fetch(marginPda);
    expect(marginData.owner.toBase58()).to.equal(provider.wallet.publicKey.toBase58());
    expect(marginData.totalCollateral.toNumber()).to.equal(0);
  });

  it("Deposits collateral into the margin engine", async () => {
    const depositAmount = new anchor.BN(500_000_000); // 500 USDC

    await program.methods
      .depositCollateral(depositAmount)
      .accounts({
        marginAccount: marginPda,
        userTokenAccount: userTokenAccount,
        vault: vaultPda,
        owner: provider.wallet.publicKey,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .rpc();

    const marginData = await program.account.marginAccount.fetch(marginPda);
    expect(marginData.totalCollateral.toNumber()).to.equal(depositAmount.toNumber());
  });

  it("Rejects trade execution if oracle slot is stale", async () => {
    // Write stale oracle mock data (Slot = 1, Price = 150_000_000 [$150])
    const oracleAccount = Keypair.generate();
    const staleSlot = 1;
    const price = 150_000_000;

    const buffer = Buffer.alloc(64);
    buffer.writeBigInt64LE(BigInt(price), 0);
    buffer.writeBigUInt64LE(BigInt(staleSlot), 8);

    const rentLamports = await provider.connection.getMinimumBalanceForRentExemption(64);
    const createTx = new anchor.web3.Transaction().add(
      SystemProgram.createAccount({
        fromPubkey: provider.wallet.publicKey,
        newAccountPubkey: oracleAccount.publicKey,
        space: 64,
        lamports: rentLamports,
        programId: program.programId,
      })
    );
    await provider.sendAndConfirm(createTx, [oracleAccount]);

    try {
      await program.methods
        .openPosition(new anchor.BN(10_000_000), new anchor.BN(160_000_000))
        .accounts({
          marginAccount: marginPda,
          market: marketPda,
          oracle: oracleAccount.publicKey,
          owner: provider.wallet.publicKey,
        })
        .rpc();
      expect.fail("Expected OracleStale error was not thrown");
    } catch (err: any) {
      expect(err.toString()).to.include("OracleStale");
    }
  });
});