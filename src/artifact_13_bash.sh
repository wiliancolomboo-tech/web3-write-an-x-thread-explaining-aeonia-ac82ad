# Switch to Devnet
solana config set --url https://api.devnet.solana.com

# Deploy the binary to Devnet
anchor deploy --provider.cluster devnet

# Verify the program binary account status
solana program show Aeon111111111111111111111111111111111111111