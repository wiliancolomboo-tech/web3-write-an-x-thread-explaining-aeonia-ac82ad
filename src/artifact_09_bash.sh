# 1. Build the Anchor program binary
anchor build

# 2. Extract program address and synchronize declare_id!
anchor keys sync

# 3. Run the complete automated test matrix
anchor test