# 1. Clone repository and change directory
git clone https://github.com/aeonian-trade/aeonian-perps.git
cd aeonian-perps

# 2. Install workspace dependencies
yarn install

# 3. Build Anchor program with release flags
anchor build

# 4. Generate localnet keypair (if not present)
solana-keygen new --no-bip39-passphrase --outfile ~/.config/solana/id.json

# 5. Start local validator with Pyth Program clone
solana-test-validator \
  --bpf-program FsJ3A3u2vn5cTVofAjvy6y5kwABJAqYWpe4975bi2epH tests/fixtures/pyth_program.so \
  --reset &

# 6. Execute full automated test suite (Unit + Integration + Liquidation test)
anchor test --skip-local-validator