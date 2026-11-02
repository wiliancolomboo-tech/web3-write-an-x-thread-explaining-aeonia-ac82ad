# Terminal 1: Spin up local validator with clone of Pyth price feeds if desired
solana-test-validator --reset

# Terminal 2: Deploy program binary
solana program deploy \
    --program-id target/deploy/aeonian_trade-keypair.json \
    target/deploy/aeonian_trade.so \
    --url localhost

# Verify deployment status
solana program show Aeon1111111111111111111111111111111111111111 --url localhost