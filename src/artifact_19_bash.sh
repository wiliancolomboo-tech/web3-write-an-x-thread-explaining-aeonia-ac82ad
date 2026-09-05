solana-test-validator --reset &
anchor deploy
solana logs | grep "consumed"