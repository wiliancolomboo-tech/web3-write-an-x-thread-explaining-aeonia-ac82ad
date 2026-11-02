#[account(zero_copy)]
#[repr(C)]
pub struct Position {
    pub owner: Pubkey,            // 32
    pub size: u64,                 // 8
    pub collateral: u64,           // 8
    pub entry_price: u64,          // 8
    pub last_updated_slot: u64,    // 8
    pub is_long: u8,               // 1
    pub _reserved: [u8; 7],        // 7 (8-byte alignment)
}