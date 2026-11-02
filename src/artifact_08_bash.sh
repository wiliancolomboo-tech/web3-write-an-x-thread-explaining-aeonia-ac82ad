# Initialize project directory
mkdir -p aeonian-trade-protocol
cd aeonian-trade-protocol
anchor init aeonian_trade --javascript false

# Replace the program code and test files
cp path/to/lib.rs programs/aeonian_trade/src/lib.rs
cp path/to/aeonian-trade.spec.ts tests/aeonian-trade.spec.ts

# Install required SPL token dependencies
yarn add @solana/spl-token @coral-xyz/anchor chai mocha @types/chai @types/mocha