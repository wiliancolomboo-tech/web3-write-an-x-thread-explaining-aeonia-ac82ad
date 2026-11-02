# 1. Clone repository and navigate to workspace
git clone https://github.com/aeonian-trade/protocol.git
cd protocol

# 2. Install NPM dependencies
yarn install

# 3. Build Anchor program and generate IDL / TypeScript types
anchor build

# 4. Verify Program ID alignment
# Get the keypair pubkey
solana address -k target/deploy/aeonian_trade-keypair.json
# Ensure the printed pubkey matches declare_id!("...") in programs/aeonian-trade/src/lib.rs

# 5. Run the full unit & integration QA test suite on localnet
anchor test