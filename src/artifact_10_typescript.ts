import { TwitterApi } from "twitter-api-v2";
import * as dotenv from "dotenv";
dotenv.config();

const client = new TwitterApi({
  appKey: process.env.TWITTER_API_KEY!,
  appSecret: process.env.TWITTER_API_SECRET!,
  accessToken: process.env.TWITTER_ACCESS_TOKEN!,
  accessSecret: process.env.TWITTER_ACCESS_SECRET!,
});

const thread = [
  `🧵 1/11 High-throughput DeFi on @Solana is evolving beyond conventional AMMs.\n\nIntroducing @AeonianTrade: an autonomous, capital-efficient, zero-slippage perpetual & margin trading protocol leveraging asynchronous execution, Pyth Network pull oracles, and isolated debt-pools.\n\nHere’s an in-depth breakdown of how it works under the hood 👇 ⚡`,
  `2/11 Most decentralized perpetual exchanges suffer from:\n❌ Extreme LVR (Loss-Versus-Rebalancing)\n❌ Front-running and latency arbitrage on stale oracle feeds\n❌ Cascade liquidations wiping out LP collateral\n\nAeonian Trade re-engineers this via a dynamic dynamic skew-hedged vault model.`,
  `3/11 **The Vault Architecture**:\nInstead of unified risk pools vulnerable to contagion, Aeonian isolates risk via collateral-specific Program Derived Addresses (PDAs).\n\n• Multi-asset collateral: $SOL, $USDC, $mSOL\n• Dynamic skew fees penalizing one-sided market buildup\n• Sub-second settlement via Sealevel parallel processing`,
  `4/11 Sub-Second Oracle Verification (Pyth Pull Feeds):\n\n🔹 Oracles are pushed into the transaction payload atomically\n🔹 Stale tolerance checked on-chain (publish_time >= clock.unix_timestamp - 30s)\n🔹 Price confidence intervals (σ) factored into liquidation calculations directly in Rust`,
  `5/11 Liquidations shouldn't crash order books. Aeonian implements fractional, Dutch-auction-driven liquidations:\n\n📉 When Health Factor < 1.0, positions are liquidated in 20% tranches rather than 100% full-margin dumps.\n🛡️ Keepers receive prioritized Jito MEV-bundle rebates for stabilizing underwater vaults.`,
  `6/11 High-frequency perps demand extreme CU efficiency.\n\nAeonian consumes < 48,000 Compute Units per execution:\n⚡ Zero dynamic heap allocations in execution paths\n⚡ Zero-copy deserialization via AccountLoader\n⚡ Bitwise position tracking for multiple open trade legs`,
  `7/11 Developers can integrate directly with Aeonian via Anchor CPIs:\n\n📦 Flash-minting liquidity for arbitrageurs\n📦 Delta-neutral automated yield vaults\n📦 Dynamic hedging primitives for tokenized treasuries\n\nAll accounts are bound to strict cryptographic seed derivations.`,
  `8/11 🚨 ANNOUNCING: Aeonian Technical Livestream Series 🚨\n\nWe're taking you deep into the smart contracts, keeper architectures, and automated trading interfaces across two interactive technical deep dives!\n\nMark your calendars 🗓️:\n🔴 Session 1: Architecture & Live Smart Contract Audit Walkthrough\n🔴 Session 2: Building Automated Keepers & Dynamic Yield Strategies`,
  `9/11 📅 Session 1: Under the Hood of Aeonian Protocol\n• Date: Thursday, Oct 24th @ 16:00 UTC\n• Focus: Rust codebase breakdown, PDA isolation, oracle verification, and CU profiling.\n\n📅 Session 2: Building Bot Infrastructure & Yield Systems\n• Date: Tuesday, Oct 29th @ 17:00 UTC\n• Focus: Typescript Keeper Bots, Jito MEV bundling, and Delta-Neutral LP strategies.`,
  `10/11 Want early access to the Devnet testnet and Keeper Bug Bounty Program?\n\n🔗 Whitepaper & System Specs: https://docs.aeonian.trade\n🔗 GitHub Reference Repository: https://github.com/aeonian-trade/protocol-core\n🔗 Livestream Registration: https://lu.ma/aeonian-trade-livestreams`,
  `11/11 The future of decentralized perps requires institutional performance on Solana’s permissionless rails.\n\n💬 Drop your questions below for the engineering team to address during Session 1!\n\nRT and like the first tweet to spread high-performance DeFi across the ecosystem. 🌐🚀`
];

async function publish() {
  console.log("Validating thread payload lengths...");
  for (const [index, tweet] of thread.entries()) {
    if (tweet.length > 280) {
      throw new Error(`Tweet index ${index} exceeds 280 character limit: ${tweet.length} chars`);
    }
  }

  console.log(`Publishing ${thread.length} tweets to X...`);
  const response = await client.v2.tweetThread(thread);
  console.log("Thread successfully posted! Thread root ID:", response[0].data.id);
}

publish().catch(console.error);