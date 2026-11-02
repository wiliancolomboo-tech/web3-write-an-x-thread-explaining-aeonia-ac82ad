# 1. Install Node.js dependencies
yarn install

# 2. Build the Anchor program to generate IDL and TypeScript typings
anchor build

# 3. Synchronize program keys
anchor keys sync

# 4. Start local test validator and run complete TypeScript QA suite
anchor test