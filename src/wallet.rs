use solana_sdk::{
    signature::Keypair,
    signer::Signer,
};

pub struct WalletManager {
    pub keypair: Keypair,
}

impl WalletManager {
    /// Load keypair from a base58-encoded private key string (Phantom export format).
    pub fn new(private_key_base58: &str) -> Result<Self, String> {
        let keypair = Keypair::from_base58_string(private_key_base58);
        Ok(Self { keypair })
    }

    /// Generate a random keypair for testing/simulation.
    pub fn random() -> Self {
        Self { keypair: Keypair::new() }
    }

    pub fn pubkey(&self) -> solana_sdk::pubkey::Pubkey {
        self.keypair.pubkey()
    }
}
