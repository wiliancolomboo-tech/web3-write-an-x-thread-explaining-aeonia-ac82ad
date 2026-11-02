import os
import tweepy

def publish_aeonian_thread():
    api_key = os.getenv("TWITTER_API_KEY")
    api_secret = os.getenv("TWITTER_API_SECRET")
    access_token = os.getenv("TWITTER_ACCESS_TOKEN")
    access_token_secret = os.getenv("TWITTER_ACCESS_TOKEN_SECRET")

    client = tweepy.Client(
        consumer_key=api_key,
        consumer_secret=api_secret,
        access_token=access_token,
        access_token_secret=access_token_secret
    )

    tweets = [
        "1/11 ⚡ High-throughput derivatives on-chain have historically suffered from front-running, toxic MEV, and stale oracle exploits.\n\nThat era ends now.\n\nMeet @AeonianTrade: Institutional-grade perpetuals powered by @Solana’s SeaLevel runtime and Pyth Oracles. 🧵👇",
        "2/11 Most perp DEXs duplicate order books or rely on constant-product AMMs that bleed LP capital to toxic arbitrageurs.\n\nAeonian redesigns the stack around a unified, multi-asset liquidity pool: the $ALP.",
        "3/11 Trades execute against the global vault using dynamic virtual pricing:\n🔹 Zero price impact on entry\n🔹 Real-time delta neutralization\n🔹 Protocol-backed organic yield",
        "4/11 Stale oracle attacks break perp DEXs during volatile market swings.\n\nAeonian natively implements Pyth low-latency price feeds directly inside the execution instruction with max confidence interval enforcement.",
        "5/11 Solana's account-access list runtime means user accounts operate as independent PDAs: contention is isolated per trader-market pair without global state lockups.",
        "6/11 Solvency is maintained by automated liquidation cranks checking:\nHealth Factor = (Collateral ± PnL) / Maintenance Margin.\nKeepers earn 2.5% rebates on deterministic liquidations.",
        "7/11 Security is non-negotiable: Formal verification on mathematical invariant bounds, zero arbitrary CPI destinations, and PDA vault isolation.",
        "8/11 Open-source Rust keeper SDKs, low-latency TypeScript order routers, and WebSocket APIs are now live for algorithmic market makers.",
        "9/11 🎙️ ANNOUNCING: The Aeonian Engineering Livestream Series!\n\n📅 Ep 1: Thursday, 18:00 UTC - 'Hacking Perpetual Solvency on Solana'\n📅 Ep 2: Next Tuesday, 18:00 UTC - 'Writing High-Performance Keeper Bots in Rust'",
        "10/11 Livestream attendees get early access to our Devnet Trading Arena with a $50,000 $USDC prize pool.",
        "11/11 The future of decentralized finance demands millisecond execution and zero counterparty risk.\n\n⚡ Test app: https://app.aeonian.trade\n📖 Docs: https://docs.aeonian.trade"
    ]

    last_tweet_id = None
    for idx, text in enumerate(tweets):
        if idx == 0:
            response = client.create_tweet(text=text)
        else:
            response = client.create_tweet(text=text, in_reply_to_tweet_id=last_tweet_id)
        
        last_tweet_id = response.data['id']
        print(f"Posted Tweet {idx + 1}/11 [ID: {last_tweet_id}]")

if __name__ == "__main__":
    publish_aeonian_thread()