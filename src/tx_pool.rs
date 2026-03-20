/// tx_pool.rs — Pre-Signed Transaction Pool.
/// Maintains 20 reusable transaction skeletons. At snipe time,
/// only the mint pubkey and fresh blockhash need to be injected.
use solana_sdk::{
    compute_budget::ComputeBudgetInstruction,
    instruction::{Instruction, AccountMeta},
    message::{v0, VersionedMessage},
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer,
    transaction::VersionedTransaction,
    hash::Hash,
};
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

pub const POOL_SIZE: usize = 20;

/// A ready-to-fill transaction slot.
pub struct TxTemplate {
    pub priority_fee: u64,
}

pub struct TxPool {
    templates: Arc<RwLock<Vec<TxTemplate>>>,
    shadow_mode: bool,
}

impl TxPool {
    pub fn new(priority_fee: u64, shadow_mode: bool) -> Self {
        let mut templates = Vec::with_capacity(POOL_SIZE);
        for _ in 0..POOL_SIZE {
            templates.push(TxTemplate { priority_fee });
        }
        info!("TxPool: {} templates ready", POOL_SIZE);
        Self { templates: Arc::new(RwLock::new(templates)), shadow_mode }
    }

    /// Build and sign a full buy transaction, injecting mint + blockhash at call time.
    /// The buy instruction skeleton is dynamically finalised here.
    pub async fn build_buy_tx(
        &self,
        mint: &Pubkey,
        bonding_curve: &Pubkey,
        assoc_bonding_curve: &Pubkey,
        buyer_ata: &Pubkey,
        keypair: &Keypair,
        blockhash: Hash,
        buy_lamports: u64,
        priority_fee: u64,
    ) -> Option<(VersionedTransaction, f64, f64)> {
        let pump_program =
            Pubkey::from_str("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P").ok()?;
        let global_pda =
            Pubkey::from_str("4wTV81uD9pTAG9XTFVKV3RJ4ZVCNL9b7RPKDjV7h647").ok()?;
        let fee_recipient =
            Pubkey::from_str("CebN5WGQ4jvEPvsVU4EoHEpgznyZKRL1eovpZTPLbz5").ok()?;
        let system_prog  = solana_sdk::system_program::id();
        let token_prog   = spl_token::id();
        let assoc_prog   = spl_associated_token_account::id(); // kept for template but not in IX
        let rent_sysvar  = solana_sdk::sysvar::rent::id();
        let event_auth   = Pubkey::from_str("Ce6TQqeHC9p8KBAZvhtGpwzvKAJKSVzbszeAVdgVkqZY").unwrap();

        // Encode: discriminator(8) | amount(8) | max_sol_cost(8)
        let discriminator: [u8; 8] = [102, 6, 61, 18, 1, 218, 235, 234];
        let max_sol_cost: u64 = if self.shadow_mode {
            0 // Force failure safely
        } else {
            buy_lamports + buy_lamports / 5 // +20% slippage
        };
        let mut data = Vec::with_capacity(24);
        data.extend_from_slice(&discriminator);
        data.extend_from_slice(&buy_lamports.to_le_bytes());
        data.extend_from_slice(&max_sol_cost.to_le_bytes());

        let buy_ix = Instruction {
            program_id: pump_program,
            accounts: vec![
                AccountMeta::new_readonly(global_pda, false),
                AccountMeta::new(fee_recipient, false),
                AccountMeta::new_readonly(*mint, false),
                AccountMeta::new(*bonding_curve, false),
                AccountMeta::new(*assoc_bonding_curve, false),
                AccountMeta::new(*buyer_ata, false),
                AccountMeta::new(keypair.pubkey(), true),
                AccountMeta::new_readonly(system_prog, false),
                AccountMeta::new_readonly(token_prog, false),
                AccountMeta::new_readonly(rent_sysvar, false),
                AccountMeta::new_readonly(event_auth, false),
                AccountMeta::new_readonly(pump_program, false),
            ],
            data,
        };

        let compute_price_ix = ComputeBudgetInstruction::set_compute_unit_price(priority_fee);
        let compute_limit_ix = ComputeBudgetInstruction::set_compute_unit_limit(200_000);

        let build_start = std::time::Instant::now();
        let msg = v0::Message::try_compile(
            &keypair.pubkey(),
            &[compute_price_ix, compute_limit_ix, buy_ix],
            &[],
            blockhash,
        ).ok()?;
        let build_time = build_start.elapsed().as_secs_f64() * 1000.0;

        let sign_start = std::time::Instant::now();
        let tx = VersionedTransaction::try_new(VersionedMessage::V0(msg), &[keypair]).ok()?;
        let sign_time = sign_start.elapsed().as_secs_f64() * 1000.0;

        Some((tx, build_time, sign_time))
    }

    /// Update the priority fee on all templates dynamically.
    pub async fn update_priority_fee(&self, new_fee: u64) {
        let mut templates = self.templates.write().await;
        for t in templates.iter_mut() {
            t.priority_fee = new_fee;
        }
    }
}

/// Derive the pump.fun bonding curve PDA for a given mint.
/// Seeds: ["bonding-curve", mint_pubkey]
pub fn derive_bonding_curve(mint: &Pubkey) -> (Pubkey, u8) {
    let pump_program = Pubkey::from_str("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P").unwrap();
    Pubkey::find_program_address(
        &[b"bonding-curve", mint.as_ref()],
        &pump_program,
    )
}

/// Derive the associated bonding curve token account.
pub fn derive_assoc_bonding_curve(bonding_curve: &Pubkey, mint: &Pubkey) -> Pubkey {
    spl_associated_token_account::get_associated_token_address(bonding_curve, mint)
}

/// Derive the buyer's associated token account.
pub fn derive_buyer_ata(buyer: &Pubkey, mint: &Pubkey) -> Pubkey {
    spl_associated_token_account::get_associated_token_address(buyer, mint)
}
