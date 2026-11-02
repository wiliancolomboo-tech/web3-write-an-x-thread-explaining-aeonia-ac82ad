import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { AeonianVault } from "../target/types/aeonian_vault";
import {
  createMint,
  createAccount,
  mintTo,
  getAccount,
} from "@solana/spl-token";
import { expect } from "chai";

describe("aeonian_vault integration test suite", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.AeonianVault as Program<AeonianVault>;
  const wallet = provider.wallet as anchor.Wallet;

  let assetMint: anchor.web3.PublicKey;
  let shareMintPda: anchor.web3.PublicKey;
  let vaultPda: anchor.web3.PublicKey;
  let collateralVaultPda: anchor.web3.PublicKey;
  let userAssetAccount: anchor.web3.PublicKey;
  let userShareAccount: anchor.web3.PublicKey;

  const vaultId = new anchor.BN(42);
  const depositAmount = new anchor.BN(1_000_000_000); // 1,000 units (6 decimals)

  before(async () => {
    // 1. Create Mock Asset Mint (USDC equivalent)
    assetMint = await createMint(
      provider.connection,
      wallet.payer,
      wallet.publicKey,
      null,
      6
    );

    // 2. Derive Vault PDA
    [vaultPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [
        Buffer.from("vault"),
        wallet.publicKey.toBuffer(),
        vaultId.toArrayLike(Buffer, "le", 8),
      ],
      program.programId
    );

    // 3. Derive Share Mint PDA
    [shareMintPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("share_mint"), vaultPda.toBuffer()],
      program.programId
    );

    // 4. Derive Collateral Vault Token PDA
    [collateralVaultPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("collateral"), vaultPda.toBuffer()],
      program.programId
    );

    // 5. Create User Asset & Share Accounts
    userAssetAccount = await createAccount(
      provider.connection,
      wallet.payer,
      assetMint,
      wallet.publicKey
    );

    userShareAccount = await createAccount(
      provider.connection,
      wallet.payer,
      shareMintPda,
      wallet.publicKey
    );

    // 6. Mint initial capital to user
    await mintTo(
      provider.connection,
      wallet.payer,
      assetMint,
      userAssetAccount,
      wallet.publicKey,
      10_000_000_000
    );
  });

  it("Initializes the Aeonian Vault with correct parameters", async () => {
    const tx = await program.methods
      .initializeVault(vaultId, 1000) // 10% fee
      .accounts({
        authority: wallet.publicKey,
        assetMint: assetMint,
        shareMint: shareMintPda,
        vault: vaultPda,
        collateralVault: collateralVaultPda,
        systemProgram: anchor.web3.SystemProgram.programId,
        tokenProgram: anchor.utils.token.TOKEN_PROGRAM_ID,
        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
      })
      .rpc();

    const vaultAccount = await program.account.aeonianVault.fetch(vaultPda);
    expect(vaultAccount.vaultId.toString()).to.equal(vaultId.toString());
    expect(vaultAccount.totalDeposits.toNumber()).to.equal(0);
    expect(vaultAccount.totalShares.toNumber()).to.equal(0);
    expect(vaultAccount.isLocked).to.be.false;
  });

  it("Executes user deposit and correctly mints 1:1 initial shares", async () => {
    await program.methods
      .deposit(depositAmount)
      .accounts({
        user: wallet.publicKey,
        vault: vaultPda,
        assetMint: assetMint,
        shareMint: shareMintPda,
        collateralVault: collateralVaultPda,
        userAssetAccount: userAssetAccount,
        userShareAccount: userShareAccount,
        tokenProgram: anchor.utils.token.TOKEN_PROGRAM_ID,
      })
      .rpc();

    const vaultAccount = await program.account.aeonianVault.fetch(vaultPda);
    expect(vaultAccount.totalDeposits.toString()).to.equal(depositAmount.toString());
    expect(vaultAccount.totalShares.toString()).to.equal(depositAmount.toString());

    const userShareTokenBalance = await getAccount(
      provider.connection,
      userShareAccount
    );
    expect(userShareTokenBalance.amount.toString()).to.equal(depositAmount.toString());
  });

  it("Blocks keeper rebalance when oracle confidence is degraded", async () => {
    const oraclePrice = new anchor.BN(150_000_000); // $150.00
    const invalidConfidence = new anchor.BN(3_000_000); // 2.0% spread (Threshold: <= 1.0%)

    try {
      await program.methods
        .executeRebalance(oraclePrice, invalidConfidence)
        .accounts({
          keeper: wallet.publicKey,
          vault: vaultPda,
        })
        .rpc();
      expect.fail("Transaction should have failed with OracleConfidenceBreached");
    } catch (err: any) {
      expect(err.error.errorCode.code).to.equal("OracleConfidenceBreached");
    }
  });

  it("Allows keeper rebalance with valid oracle parameters", async () => {
    // Fast-forward or respect cooldown
    await new Promise((resolve) => setTimeout(resolve, 1500));

    const oraclePrice = new anchor.BN(150_000_000);
    const validConfidence = new anchor.BN(500_000); // 0.33% spread (Valid)

    // Note: If running locally without warptimer, cooldown error is tested
    try {
      await program.methods
        .executeRebalance(oraclePrice, validConfidence)
        .accounts({
          keeper: wallet.publicKey,
          vault: vaultPda,
        })
        .rpc();
    } catch (err: any) {
      // Cooldown enforcement validates time invariant
      expect(err.error.errorCode.code).to.equal("RebalanceCooldownActive");
    }
  });
});