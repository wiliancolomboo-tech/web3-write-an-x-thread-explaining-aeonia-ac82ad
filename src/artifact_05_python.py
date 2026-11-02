#!/usr/bin/env python3
"""
Aeonian Trade - Autonomous X Thread & Livestream Broadcast Bot
Deploys high-fidelity technical threads directly to X API v2.
"""

import os
import sys
import time
from typing import List
import tweepy

TWEETS: List[str] = [
    "Most perpetual DEXs on Solana are built on an unstable lie: LPs are treated as exit liquidity for toxic flow, while collateral sits idle earning zero baseline APY.\n\nThat changes today.\n\nMeet @AeonianTrade: The first yield-bearing perpetual engine built on Solana utilizing zero-impact virtual liquidity pools and dynamic delta hedging.\n\nA technical deep dive 🧵👇 (1/12)",
    "Legacy perps (vAMMs and isolated pools) suffer from:\n1. Fragmented depth: Thin order books lead to massive slippage on large fills.\n2. Idle TVL: Deposited LP capital remains stagnant, missing out on Solana’s ~7% staking yields.\n3. Asymmetric toxic flow: Arbitrageurs front-run stale oracle slots during volatility. (2/12)",
    "Aeonian solves this through the Aeonian Liquidity Pool (ALP).\nALP functions as an index basket (SOL, BTC, ETH, USDC).\n\nInstead of locking capital inside an isolated AMM state:\n• 70% of delta-neutral reserves earn native yield across LSTs & Kamino\n• 30% remains liquid for instant margin settlement. (3/12)",
    "How does Aeonian execute 7-figure swaps with zero price impact?\n\nThrough virtualized counterparty settlement:\nTraders don't match against an order book counter-party. They trade directly against the aggregate pool using deterministic Pyth Network Low-Latency Oracles.\n\nEntry Price = Instantaneous Oracle Mid-Price ± Dynamic Borrow/Skew Fee. (4/12)",
    "When Long OI > Short OI, protocol solvency risk rises.\n\nAeonian implements an automated dynamic skew rate:\nFunding_Rate_PerHour = (Net_Skew / Total_Pool_Depth) * K_velocity\n\nThis dynamically penalizes the heavy side and heavily subsidizes counter-trend arb positions, naturally restoring pool balance without keeper intervention. (5/12)",
    "Solana block times demand sub-second liquidation precision.\n\nAeonian uses atomic instruction bundling:\nLiquidators construct atomic bundles containing:\n1. compute_budget::set_compute_unit_limit\n2. pyth::price_update\n3. aeonian::liquidate_position\n\nIf collateral ratio < Maintenance Margin (1.5%), position unwinds in a single slot. No bad debt. Ever. (6/12)",
    "Our smart contracts are built for raw speed:\n⚡ Compute units per trade: < 32,000 CU\n⚡ Zero-Copy account deserialization (AccountLoader<'info, MarketState>)\n⚡ PDA-derived vault authorities eliminating cross-program risk\n⚡ Deterministic arithmetic using FixedPoint math with overflow checks. (7/12)",
    "Why provide liquidity to ALP?\n1. 100% of borrow fees paid by leveraged traders\n2. 70% of protocol liquidation penalties\n3. Underlying base APY from Solana LST yields (JitoSOL/mSOL)\n\nLPs don't just act as the house—they capture triple-compounded yield on their principal. (8/12)",
    "Security is non-negotiable:\n🔒 Reentrancy guards enforced at the runtime level\n🔒 Strict staleness threshold on Pyth Price feeds (< 5 seconds / confidence interval checks)\n🔒 Invariant checks: Total_Assets >= Borrowed_Assets + Locked_Collateral enforced pre/post every single CPI instruction. (9/12)",
    "Want to see the engine in action?\n\nWe are launching the Aeonian Masterclass Livestreams! 🔴\n📅 Episode 1: 'Deconstructing Low-Latency Liquidations on Solana'\n🗓️ Thursday, 18:00 UTC\n📍 Live on X Spaces & YouTube Live\n\nSet your reminders now: https://livestream.aeonian.trade/ep1 (10/12)",
    "Ahead of our Mainnet Launch, we are opening our Developer & Keeper Incentive Program:\n🛠️ $100k+ in bounties for open-source keeper bots, custom UI frontends, and Dune analytics dashboards.\nFull Rust SDK + TypeScript clients are open-sourced today on GitHub: https://github.com/aeonian-trade/protocol (11/12)",
    "The future of Solana trading isn't centralized off-chain order books masquerading as DeFi.\n\nIt is high-performance, transparent, yield-bearing decentralized perpetual infrastructure.\n\nFollow @AeonianTrade, RT the first tweet, join our Discord, and don't miss Thursday's livestream! 🌊🚀 (12/12)"
]

def publish_thread():
    api_key = os.getenv("TWITTER_API_KEY")
    api_secret = os.getenv("TWITTER_API_SECRET")
    access_token = os.getenv("TWITTER_ACCESS_TOKEN")
    access_token_secret = os.getenv("TWITTER_ACCESS_TOKEN_SECRET")

    if not all([api_key, api_secret, access_token, access_token_secret]):
        print("[!] Missing Twitter API Environment Credentials.")
        print("[+] Mock Mode: Validating Thread Lengths and Format:")
        for idx, tweet in enumerate(TWEETS, 1):
            assert len(tweet) <= 280, f"Tweet {idx} exceeds 280 characters: {len(tweet)}"
            print(f"[Tweet {idx:02d} OK - {len(tweet)} chars]: {tweet[:45]}...")
        print("[SUCCESS] All 12 tweets validated under the 280 character limit.")
        return

    client = tweepy.Client(
        consumer_key=api_key,
        consumer_secret=api_secret,
        access_token=access_token,
        access_token_secret=access_token_secret
    )

    last_tweet_id = None
    for idx, tweet_text in enumerate(TWEETS, 1):
        print(f"Posting tweet {idx}/{len(TWEETS)}...")
        response = client.create_tweet(
            text=tweet_text,
            in_reply_to_tweet_id=last_tweet_id
        )
        last_tweet_id = response.data['id']
        print(f"-> Posted ID: {last_tweet_id}")
        time.sleep(2.5) # Avoid burst rate-limiting

    print("[DONE] Aeonian Trade thread successfully deployed to X.")

if __name__ == "__main__":
    publish_thread()