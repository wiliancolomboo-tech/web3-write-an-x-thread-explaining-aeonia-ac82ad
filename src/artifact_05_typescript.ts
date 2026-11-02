import { TwitterApi } from "twitter-api-v2";
import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { AeonianTrade } from "../target/types/aeonian_trade";
import { Keypair, LAMPORTS_PER_SOL, PublicKey } from "@solana/web3.js";
import {
  createMint,
  createAccount,
  mintTo,
  TOKEN_PROGRAM_ID,
} from "@solana/spl-token";
import * as assert from "assert";
import * as dotenv from "dotenv";

dotenv.config();

// 1. Tweet Payload Configuration
const THREAD_TWEETS = [
  `1/10\nDecentralized perpetuals have historically been bottlenecked by EVM latency, toxic MEV order flow, and sluggish liquidation cranks.\n\nEnter @AeonianTrade: an institutional-grade, ultra-low latency perpetual trading engine engineered natively on Solana.\n\nHere is an architectural deep dive. 👇⚡`,
  `2/10\nTraditional vAMMs separate pricing from liquidity, causing toxic skew and reserve depletion during volatile market events.\n\nAeonian eliminates this via externalized virtual liquidity routing:\n• Sub-50ms @PythNetwork push oracles\n• Isolated dynamic risk engines\n• Zero-slippage in-band execution`,
  `3/10\nUnder the hood: Solana’s Sealevel runtime enables Aeonian to eliminate global order book contention.\n\nEvery trader’s position is an isolated PDA:\nseeds = [b"position", user_pubkey, market_id]\n\nTrades execute concurrently without locking cross-trader states. High throughput, zero congestion.`,
  `4/10\nLet’s talk liquidation dynamics.\n\nEVM protocols rely on asynchronous multi-block Dutch auctions—frequently ending in bad debt during cascading crashes.\n\nAeonian features Crankless JIT liquidations: Sub-slot execution directly through Flash Fillers at < 35k Compute Units.`,
  `5/10\nCapital Efficiency: Aeonian Liquidity Pool (ALP).\n\nLPs deposit USDC, SOL, or BTC into a unified, delta-hedged solvency vault.\nALP acts as the dynamic counterparty, streaming auto-compounding borrow rates, swap fees, and liquidation bounties directly to LPs.`,
  `6/10\nSecurity first. The core smart contracts enforce:\n🛡️ Strict Reentrancy Transience\n🛡️ Strict Oracle Staleness Guards (stale threshold: <= 5 slots)\n🛡️ Account ownership & discriminator verification via zero-copy deserialization\n🛡️ Zero reliance on unconstrained CPIs`,
  `7/10\nDevelopers & Quant Traders: Aeonian provides a low-overhead Rust SDK and TypeScript client for direct RPC integration.\n\nExecute complex algorithmic delta-neutral strategies, programmatic TWAP/limit orders, or high-frequency market-making natively on-chain.`,
  `8/10\nWant to see the code, review live performance benchmarks, and run localnet simulations?\n\nWe are launching the Aeonian Trade Developer & Trader Livestream Series! 🎙️\n\nGet ready for deep-dive terminal sessions, live contract audits, and LP profitability modeling.`,
  `9/10\n📅 LIVESTREAM SCHEDULE:\n📍 Session 1: "Architecting Ultra-Low Latency Derivatives on Solana"\n   • Deep dive into PDAs, Sealevel optimization & Pyth oracles.\n📍 Session 2: "Algorithmic Arbitrage & JIT Liquidator Bots"\n   • Live coding a sub-50ms liquidation bot in Rust.\n\n🔔 Set reminders: https://aeonian.trade/livestreams`,
  `10/10\nThe future of institutional DeFi trading requires CEX-tier speed combined with uncompromising on-chain custody and transparency.\n\nAeonian Trade delivers on that promise.\n\nRead the technical docs, inspect the contracts, and join our upcoming livestream.\n\nRT + Follow @AeonianTrade! 🚀`,
];

// 2. On-Chain QA & Integration Verification Suite
async function runQAIntegrationVerification() {
  console.log("==================================================");
  console.log("▶ [SOLANA LOCALNET] Verifying Aeonian Smart Contract Logic");
  console.log("==================================================");

  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);
  const program = anchor.workspace.AeonianTrade as Program<AeonianTrade>;

  const authority = Keypair.generate();
  const trader = Keypair.generate();
  const liquidator = Keypair.generate();

  // Airdrop lamports
  for (const kp of [authority, trader, liquidator]) {
    const sig = await provider.connection.requestAirdrop(
      kp.publicKey,
      2 * LAMPORTS_PER_SOL
    );
    await provider.connection.confirmTransaction(sig);
  }

  // Create Collateral Token Mint & Accounts
  const mintAuthority = Keypair.generate();
  const collateralMint = await createMint(
    provider.connection,
    authority,
    mintAuthority.publicKey,
    null,
    6
  );

  const [globalConfigPda] = PublicKey.findProgramAddressSync(
    [Buffer.from("global-config")],
    program.programId
  );

  const [collateralVaultPda] = PublicKey.findProgramAddressSync(
    [Buffer.from("vault"), collateralMint.toBuffer()],
    program.programId
  );

  // Initialize Global Config
  await program.methods
    .initializeGlobalConfig(authority.publicKey, 100) // 1% fee
    .accounts({
      globalConfig: globalConfigPda,
      authority: authority.publicKey,
      systemProgram: anchor.web3.SystemProgram.programId,
    })
    .signers([authority])
    .rpc();

  console.log("✔ GlobalConfig initialized successfully.");

  // Trader token setup
  const traderTokenAccount = await createAccount(
    provider.connection,
    trader,
    collateralMint,
    trader.publicKey
  );
  await mintTo(
    provider.connection,
    authority,
    collateralMint,
    traderTokenAccount,
    mintAuthority,
    1_000_000_000
  );

  const marketId = new anchor.BN(1);
  const [positionPda] = PublicKey.findProgramAddressSync(
    [
      Buffer.from("position"),
      trader.publicKey.toBuffer(),
      marketId.toArrayLike(Buffer, "le", 8),
    ],
    program.programId
  );

  // Open Position: 100 USDC Collateral, 10x Leverage, Long at $100.00
  const currentSlot = await provider.connection.getSlot();
  const collateralAmount = new anchor.BN(100_000_000); // 100 tokens

  await program.methods
    .openPosition(
      marketId,
      collateralAmount,
      10, // 10x leverage
      true, // is_long
      new anchor.BN(100_000_000), // $100.00 entry price
      new anchor.BN(currentSlot)
    )
    .accounts({
      position: positionPda,
      trader: trader.publicKey,
      userTokenAccount: traderTokenAccount,
      collateralMint: collateralMint,
      collateralVault: collateralVaultPda,
      globalConfig: globalConfigPda,
      tokenProgram: TOKEN_PROGRAM_ID,
      systemProgram: anchor.web3.SystemProgram.programId,
    })
    .signers([trader])
    .rpc();

  const posAccount = await program.account.position.fetch(positionPda);
  assert.strictEqual(
    posAccount.sizeUsd.toNumber(),
    1_000_000_000,
    "Position size must reflect 10x leverage"
  );
  console.log("✔ Open Position verified with correct leverage & PDA allocation.");
}

// 3. Automated X Thread Publisher
async function publishXThread() {
  console.log("\n==================================================");
  console.log("▶ [X / TWITTER API] Checking Dispatch Credentials");
  console.log("==================================================");

  const apiKey = process.env.TWITTER_API_KEY;
  const apiSecret = process.env.TWITTER_API_SECRET;
  const accessToken = process.env.TWITTER_ACCESS_TOKEN;
  const accessSecret = process.env.TWITTER_ACCESS_SECRET;

  if (!apiKey || !apiSecret || !accessToken || !accessSecret) {
    console.warn(
      "⚠ Twitter API credentials missing in environment (.env). Simulating thread dispatch safely.\n"
    );
    THREAD_TWEETS.forEach((tweet, idx) => {
      console.log(`[DRY-RUN TWEET ${idx + 1}/${THREAD_TWEETS.length}] (${tweet.length} chars):`);
      console.log(tweet);
      console.log("--------------------------------------------------");
    });
    return;
  }

  const twitterClient = new TwitterApi({
    appKey: apiKey,
    appSecret: apiSecret,
    accessToken: accessToken,
    accessSecret: accessSecret,
  });

  console.log("Broadcasting technical thread directly to X API v2...");
  let lastTweetId = "";

  for (let i = 0; i < THREAD_TWEETS.length; i++) {
    const text = THREAD_TWEETS[i];
    const res = await twitterClient.v2.tweet({
      text,
      ...(lastTweetId ? { reply: { in_reply_to_tweet_id: lastTweetId } } : {}),
    });
    lastTweetId = res.data.id;
    console.log(`✔ Published Tweet ${i + 1}/${THREAD_TWEETS.length} [ID: ${lastTweetId}]`);
    await new Promise((resolve) => setTimeout(resolve, 1500)); // Rate limit buffer
  }
  console.log("🚀 Complete Aeonian Trade Thread published successfully!");
}

async function main() {
  try {
    await runQAIntegrationVerification();
    await publishXThread();
  } catch (err) {
    console.error("Pipeline failure:", err);
    process.exit(1);
  }
}

main();