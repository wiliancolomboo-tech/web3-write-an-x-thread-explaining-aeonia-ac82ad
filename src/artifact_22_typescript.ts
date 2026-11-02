import { TwitterApi } from "twitter-api-v2";
import * as dotenv from "dotenv";
dotenv.config();

const client = new TwitterApi({
  appKey: process.env.X_API_KEY!,
  appSecret: process.env.X_API_SECRET_KEY!,
  accessToken: process.env.X_ACCESS_TOKEN!,
  accessSecret: process.env.X_ACCESS_TOKEN_SECRET!,
});

const thread = [
  `1/10\nDecentralized perpetuals on Solana demand sub-second finality, zero toxic slippage, and deterministic margin safety.\n\nEnter @AeonianTrade: A next-generation derivatives engine built from bare metal on the SVM.\n\nHere is the deep technical breakdown + our upcoming Livestream alpha. 👇⚡\n\n#Solana #DeFi #Rust #Web3`,
  `2/10\nThe problem with legacy DEX perps?\n- Off-chain orderbooks introduce centralized sequencers & MEV frontrunning.\n- Standard vAMMs suffer from invariant drift & toxic liquidation cascades.\n- Oracle delays during peak congestion lead to bad debt across margin pools.\n\nAeonian re-architects this stack entirely. 🛠️`,
  `3/10\nUnder the hood: Aeonian uses a unified Dynamic Liquidity Pool (DLP) coupled with isolated & cross-margin Program Derived Addresses (PDAs).\n\nPosition state is tracked via zero-copy deserialization:\nSeed: [b"position", user_pubkey, market_pubkey]\n\nMinimizing heap allocation to < 32KB.`,
  `4/10\nPricing & Invariants:\nInstead of raw constant-product x * y = k, Aeonian implements a dynamic virtual skew engine:\n\nP_exec = P_oracle * (1 + kappa * (OI_long - OI_short) / L_pool)\n\nWhere kappa is dynamic friction. Long/Short imbalance pays direct skew premiums back to LPs.`,
  `5/10\nFunding Rates & Fixed-Point Precision:\nFunding isn’t an hourly snapshot—it’s accumulated on-chain continuously per slot using Q64.64 fixed-point math.\n\nZero rounding dust. Zero cumulative drift.`,
  `6/10\nLiquidations: Sub-40ms execution.\nLiquidations leverage Solana’s parallelized runtime. Keepers invoke CPIs into Pyth price feeds, check position health against strict Maintenance Margin Ratios (MMR):\n\nIf Health < MMR, instant auto-deleveraging prevents protocol insolvency.`,
  `7/10\nCompute Unit (CU) Optimization:\n• Zero-copy AccountLoader on large position vectors (~12,500 CU saved).\n• Unchecked math blocks where invariants are pre-asserted (~3,200 CU saved).\n• Total order execution: ~38,400 CUs.`,
  `8/10\nSecurity & Determinism:\n🛡️ Reentrancy impossible: Single-program PDA-locked state transitions.\n🛡️ Pyth Staleness Guard: conf_interval <= max_conf and publish_time >= clock.unix_timestamp - 30s.\n🛡️ Flash-loan invariant: Collateral cannot be withdrawn within the same slot.`,
  `9/10\n🚨 ANNOUNCING: The Aeonian Trade Technical Livestream Series! 🎥\n\nJoin our core engineering team for live code walk-throughs, architecture teardowns, and mainnet stress-testing:\n\n📅 Episode 1: "SVM Perp Mechanics & Zero-Copy State Deserialization"\n🗓️ Date: Thursday, Oct 24 | 16:00 UTC`,
  `10/10\nWhether you are an LP seeking sustainable yields, an algorithmic trader targeting sub-second fills, or a Solana dev curious about SVM systems engineering:\n\nBookmark this thread, set your reminders, and join our Discord.\n\nLivestream RSVP & Docs: https://aeonian.trade\nGitHub: https://github.com/aeonian-trade\n\nRT if you’re building the future of DeFi! 🔄⚡`
];

async function publishThread() {
  try {
    console.log("Publishing Aeonian Trade Technical Thread...");
    await client.v2.tweetThread(thread);
    console.log("Thread successfully posted to X!");
  } catch (error) {
    console.error("Error publishing thread:", error);
  }
}

publishThread();