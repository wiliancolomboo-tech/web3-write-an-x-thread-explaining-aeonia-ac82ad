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

describe("aeonian-trade protocol tests", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.AeonianTrade as Program<AeonianTrade>;

  let collateralMint: anchor.web3.PublicKey;
  let userTokenAccount: anchor.web3.PublicKey;
  let liquidatorTokenAccount: anchor.web3.PublicKey;

  const marketId = new anchor.BN(101);
  const user = anchor.web3.Keypair.generate();
  const liquidator = anchor.web3.Keypair.generate();

  let marketPda: anchor.web3.PublicKey;
  let vaultPda: anchor.web3.PublicKey;
  let positionPda: anchor.web3.PublicKey;

  const PRECISION = new anchor.BN(1_000_000_000_000);

  before(async () => {
    // Fund test wallets
    await provider.connection.confirmTransaction(
      await provider.connection.requestAirdrop(
        user.pubkey,
        10 * anchor.web3.LAMPORTS_PER_SOL
      ),
      "confirmed"
    );
    await provider.connection.confirmTransaction(
      await provider.connection.requestAirdrop(
        liquidator.pubkey,
        10 * anchor.web3.LAMPORTS_PER_SOL
      ),
      "confirmed"
    );

    // Create collateral mint (USDC simulator with 6 decimals)
    collateralMint = await createMint(
      provider.connection,
      (provider.wallet as any).payer,
      provider.wallet.publicKey,
      null,
      6
    );

    // Setup Token Accounts
    userTokenAccount = await createAccount(
      provider.connection,
      (provider.wallet as any).payer,
      collateralMint,
      user.pubkey
    );

    liquidatorTokenAccount = await createAccount(
      provider.connection,
      (provider.wallet as any).payer,
      collateralMint,
      liquidator.pubkey
    );

    // Mint 10,000 USDC to user
    await mintTo(
      provider.connection,
      (provider.wallet as any).payer,
      collateralMint,
      userTokenAccount,
      provider.wallet.publicKey,
      10_000_000_000
    );

    // Derive PDAs
    [marketPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [
        Buffer.from("market"),
        collateralMint.toBuffer(),
        marketId.toArrayLike(Buffer, "le", 8),
      ],
      program.programId
    );

    [vaultPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("vault"), marketPda.toBuffer()],
      program.programId
    );

    [positionPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("position"), marketPda.toBuffer(), user.pubkey.toBuffer()],
      program.programId
    );
  });

  it("Initializes Aeonian Market and Vault PDA securely", async () => {
    await program.methods
      .initializeMarket(marketId, new anchor.BN(0))
      .accounts({
        market: marketPda,
        vault: vaultPda,
        collateralMint: collateralMint,
        authority: provider.wallet.publicKey,
        systemProgram: anchor.web3.SystemProgram.programId,
        tokenProgram: TOKEN_PROGRAM_ID,
        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
      })
      .rpc();

    const marketState = await program.account.marketState.fetch(marketPda);
    expect(marketState.marketId.toNumber()).to.equal(101);
    expect(marketState.vault.toBase58()).to.equal(vaultPda.toBase58());
  });

  it("Opens a 10x leveraged Long position within risk bounds", async () => {
    const currentSlot = await provider.connection.getSlot();
    const oraclePrice = new anchor.BN(150).mul(PRECISION); // $150.00 SOL/USD
    const collateralAmount = new anchor.BN(1_000_000_000); // 1,000 USDC
    const positionSize = new anchor.BN(66_666_666); // ~10,000 USD notional

    await program.methods
      .openPosition(
        positionSize,
        collateralAmount,
        true, // Long
        oraclePrice,
        new anchor.BN(currentSlot)
      )
      .accounts({
        market: marketPda,
        position: positionPda,
        vault: vaultPda,
        userTokenAccount: userTokenAccount,
        user: user.pubkey,
        tokenProgram: TOKEN_PROGRAM_ID,
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .signers([user])
      .rpc();

    const pos = await program.account.positionState.fetch(positionPda);
    expect(pos.owner.toBase58()).to.equal(user.pubkey.toBase58());
    expect(pos.isLong).to.be.true;
    expect(pos.size.toNumber()).to.equal(positionSize.toNumber());
  });

  it("Rejects position creation when oracle price is stale (>3 slots)", async () => {
    const currentSlot = await provider.connection.getSlot();
    const staleSlot = new anchor.BN(currentSlot - 10);
    const oraclePrice = new anchor.BN(150).mul(PRECISION);

    try {
      await program.methods
        .openPosition(
          new anchor.BN(10_000),
          new anchor.BN(100_000),
          true,
          oraclePrice,
          staleSlot
        )
        .accounts({
          market: marketPda,
          position: positionPda,
          vault: vaultPda,
          userTokenAccount: userTokenAccount,
          user: user.pubkey,
          tokenProgram: TOKEN_PROGRAM_ID,
          systemProgram: anchor.web3.SystemProgram.programId,
        })
        .signers([user])
        .rpc();
      expect.fail("Transaction should have failed due to stale oracle");
    } catch (err: any) {
      expect(err.error.errorCode.code).to.equal("StaleOraclePrice");
    }
  });

  it("Liquidates unhealthy position when equity breaches maintenance margin", async () => {
    const currentSlot = await provider.connection.getSlot();
    // Simulate massive market crash: Price drops from $150 to $50
    const crashedPrice = new anchor.BN(50).mul(PRECISION);

    const tx = await program.methods
      .liquidatePosition(crashedPrice, new anchor.BN(currentSlot))
      .accounts({
        market: marketPda,
        position: positionPda,
        vault: vaultPda,
        liquidatorTokenAccount: liquidatorTokenAccount,
        liquidator: liquidator.pubkey,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([liquidator])
      .rpc();

    expect(tx).to.be.a("string");

    const pos = await program.account.positionState.fetch(positionPda);
    expect(pos.size.toNumber()).to.equal(0);
    expect(pos.collateral.toNumber()).to.equal(0);
  });
});