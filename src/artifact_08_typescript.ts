import { TwitterApi } from "twitter-api-v2";
import * as fs from "fs";

// Reads thread copy and publishes deterministically via API v2
async function publishThread() {
  const client = new TwitterApi({
    appKey: process.env.X_API_KEY!,
    appSecret: process.env.X_API_SECRET!,
    accessToken: process.env.X_ACCESS_TOKEN!,
    accessSecret: process.env.X_ACCESS_SECRET!,
  });

  const threadTweets = [
    `1/14 🧵 The perpetual DEX landscape on Solana is evolving rapidly. While classical AMMs struggle with toxic flow and latency arbitrage, @AeonianTrade introduces a zero-slippage, high-throughput perpetual engine built natively for Sealevel.\n\nHere is a deep dive into Aeonian Trade’s architecture, math, & our upcoming Dev Livestreams! 👇⚡`,
    `2/14 🏛️ The Problem with Legacy Perpetual Protocols:\nPerps on EVM chains suffer from block latency, front-running, and expensive liquidation checks. When high volatility hits, liquidation transactions get stuck in the mempool, leading to bad debt cascading through socialized loss engines.\n\nSolana solves the speed. Aeonian solves the math.`,
    `3/14 🔬 Architectural Innovation: Hybrid vAMM + Oracle Synthetics\nAeonian decouples trade execution from direct pool-reserve balance swings.\nBy utilizing dynamic virtual liquidity curves anchored to real-time @PythNetwork low-latency price feeds, positions settle against collateral vaults with zero impermanent loss for passive LPs.`,
    `4/14 📐 Mathematical Precision: Dynamic Funding Velocity\nFunding isn't an arbitrary static tick. Aeonian computes funding continuously:\nFR = clamp((Mark_Price - Index_Price) / Index_Price, -F_max, F_max)\n\nPositions accrue funding per elapsed slot (ΔS):\nAccrued_Funding = Position_Size * FR * (Slot_Now - Last_Slot) / Annual_Slots`,
    `5/14 🛡️ Risk Mitigation & Solvency Isolation:\nEvery market maintains a segregated Insurance Fund PDA alongside isolated risk parameters:\n- Initial Margin Requirement (IMR): 5% (20x Max Leverage)\n- Maintenance Margin Requirement (MMR): 2.5%\n- Dynamic Liquidation Penalty: Tiered based on position size to prevent cascade slippage.`,
    `6/14 ⚡ Compute Unit (CU) Engineering:\nSolana's 1.4M CU transaction limit demands radical efficiency:\n- Zero-copy account deserialization using Anchor's AccountLoader\n- In-place bitwise math for fee tracking\n- Average execution footprint: ~38,200 CU per market open/close order. That’s ~0.000019 SOL per trade!`,
    `7/14 🤖 Decentralized Keeper & Liquidation Network:\nLiquidations are permissionless. When Margin Ratio < MMR:\nLiquidator bots trigger liquidate_position via CPI, claiming a 2.5% incentive bounty while routing remaining residual value to the Insurance Vault. Zero bad debt has been socialized to date.`,
    `8/14 🔒 Institutional-Grade Security Guards:\n- Reentrancy impossible through single-invocation state transitions.\n- Pyth stale-price protection: Rejects feeds older than 30 slots or with confidence intervals > 1.5% of spot.\n- PDA Seed Derivation with hardcoded protocol seeds prevents account spoofing.`,
    `9/14 📊 Developer Tooling & SDK:\nDevelopers can integrate directly via the Aeonian Typescript SDK:\n\nSub-400ms transaction execution right out of the box.`,
    `10/14 🎙️ ANNOUNCING: The Aeonian Technical Livestream Series! 🚀\nWant to see how institutional derivatives are engineered on Solana from the ground up? We are hosting a 3-part interactive masterclass on YouTube & X Spaces with our core engineering team!\n\nMark your calendars! 🗓️👇`,
    `11/14 📅 EPISODE 1: "Deconstructing Aeonian's vAMM & Anchor Core"\n🗓️ Thursday, Nov 14th | 17:00 UTC\nTopics:\n- Rust Anchor architecture breakdown\n- Zero-copy account optimization\n- Real-time Pyth CPI integration\n- Live code walkthrough & local testbed setup`,
    `12/14 📅 EPISODE 2: "Writing High-Frequency Keeper & Liquidation Bots"\n🗓️ Tuesday, Nov 19th | 17:00 UTC\nTopics:\n- Building Rust & TypeScript liquidation daemons\n- Managing RPC priority fees & Jito MEV bundles\n- Stress-testing extreme market drops & backtesting solvency`,
    `13/14 📅 EPISODE 3: "Liquidity Provisioning, Yield Synthetics & Alpha"\n🗓️ Friday, Nov 22nd | 18:00 UTC\nTopics:\n- Delta-neutral vault strategies\n- Advanced risk telemetry & LP yield analytics\n- Interactive Q&A + Bounty announcement for open-source contributors!`,
    `14/14 🌐 The future of on-chain derivatives is scalable, auditable, and sub-second.\nDon't trade on black boxes; build on deterministic code.\n\nFollow @AeonianTrade, turn notifications 🔔 on, and drop your questions below for the core dev team!\n\nRead docs & register for livestreams: aeonian.trade/livestreams 🚀🧵`,
  ];

  console.log(`Publishing ${threadTweets.length} tweets to X...`);
  const response = await client.v2.tweetThread(threadTweets);
  console.log("Thread successfully posted! Thread root ID:", response[0].data.id);
}

publishThread().catch(console.error);