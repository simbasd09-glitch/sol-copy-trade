/// sell.rs — Real-time sell monitor with bonding curve price parsing.
/// Reads virtual_sol_reserves and virtual_token_reserves from PDA account data.
use crate::rpc_pool::RpcPool;
use crate::blockhash_pool::BlockhashPool;
use crate::wallet::WalletManager;
use crate::marketcap::MarketCapManager;
use crate::tx_pool::{derive_bonding_curve, derive_buyer_ata};
use dashmap::DashMap;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    compute_budget::ComputeBudgetInstruction,
    instruction::{Instruction, AccountMeta},
    message::{v0, VersionedMessage},
    pubkey::Pubkey,
    transaction::VersionedTransaction,
};
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{error, info, warn};

/// Offset layout in pump.fun BondingCurve account (Borsh-serialized):
/// discriminator(8) | virtual_token_reserves(8) | virtual_sol_reserves(8) | ...
const VIRTUAL_TOKEN_OFFSET: usize = 8;
const VIRTUAL_SOL_OFFSET:   usize = 16;

#[derive(Clone)]
pub struct ActivePosition {
    pub mint: String,
    pub entry_price_sol: f64,   // price in SOL per token at entry
    pub entry_time: Instant,
    pub buy_lamports: u64,
}

pub struct SellMonitor {
    pub positions: Arc<DashMap<String, ActivePosition>>,
}

impl SellMonitor {
    pub fn new() -> Self {
        Self { positions: Arc::new(DashMap::new()) }
    }

    pub fn add_position(&self, mint: String, entry_price_sol: f64, buy_lamports: u64) {
        self.positions.insert(mint.clone(), ActivePosition {
            mint: mint.clone(),
            entry_price_sol,
            entry_time: Instant::now(),
            buy_lamports,
        });
        info!(mint = %mint, entry_price_sol, "Position opened");
    }

    pub async fn monitor_loop(
        &self,
        rpc_client: Arc<RpcClient>,
        rpc_pool: Arc<RpcPool>,
        blockhash_pool: BlockhashPool,
        wallet: Arc<WalletManager>,
        mc_manager: Arc<MarketCapManager>,
        metrics: Option<Arc<crate::metrics::MetricsCollector>>,
        telegram: crate::telegram::ArcTelegram,
        worker_id: usize,
    ) {
        info!("Sell monitor {} started", worker_id);
        let mut interval = tokio::time::interval(Duration::from_millis(700));

        loop {
            interval.tick().await;

            let mut to_sell: Vec<(String, String, f64)> = Vec::new();

            for entry in self.positions.iter() {
                let pos = entry.value();
                let elapsed = pos.entry_time.elapsed();

                // == 5-minute timeout ==
                if elapsed >= Duration::from_secs(300) {
                    if !rpc_pool.test_mode {
                        let mint_pk = Pubkey::from_str(&pos.mint).unwrap();
                        let buyer_ata = derive_buyer_ata(&wallet.pubkey(), &mint_pk);
                        let balance = rpc_client.get_token_account_balance(&buyer_ata).await
                            .ok()
                            .and_then(|ui| ui.amount.parse::<u64>().ok())
                            .unwrap_or(0);
                        if balance == 0 {
                            warn!("SELL SKIPPED (no balance) for {} - Removing ghost position", pos.mint);
                            if let Some(ref m) = metrics { m.inc_sell_skip(); }
                            self.positions.remove(&pos.mint);
                            continue;
                        }
                    }
                    to_sell.push((pos.mint.clone(), "timeout".to_string(), 0.0));
                    continue;
                }

                // In simulation mode, we trigger sell immediately for testing
                let current_price;
                if rpc_pool.test_mode {
                    current_price = Some(pos.entry_price_sol * 2.0); // +100% profit
                } else {
                    current_price = Self::fetch_price(&rpc_client, &pos.mint).await;
                }

                if let Some(price) = current_price {
                    let profit_pct = (price - pos.entry_price_sol) / pos.entry_price_sol;
                    tracing::info!("SellMonitor: {} profit_pct={:.2}", pos.mint, profit_pct);
                    if profit_pct >= 0.5 {
                        // REAL BALANCE VERIFICATION
                        if !rpc_pool.test_mode {
                            let (_bonding_curve, _) = derive_bonding_curve(&Pubkey::from_str(&pos.mint).unwrap());
                            let buyer_ata = derive_buyer_ata(&wallet.pubkey(), &Pubkey::from_str(&pos.mint).unwrap());
                            let balance = rpc_client.get_token_account_balance(&buyer_ata).await
                                .ok()
                                .and_then(|ui| ui.amount.parse::<u64>().ok())
                                .unwrap_or(0);
                            
                            if balance == 0 {
                                warn!("SELL SKIPPED (no balance) for {} - Removing ghost position", pos.mint);
                                if let Some(ref m) = metrics { m.inc_sell_skip(); }
                                self.positions.remove(&pos.mint);
                                continue;
                            }
                        }
                        to_sell.push((pos.mint.clone(), "profit".to_string(), profit_pct));
                    }
                }
            }

            for (mint, reason, profit_pct) in to_sell {
                self.positions.remove(&mint);
                info!(mint = %mint, reason = %reason, "Sell triggered in monitor");

                let rpc = rpc_pool.clone();
                let bh  = blockhash_pool.clone();
                let wlt = wallet.clone();
                let mcm = mc_manager.clone();
                let rpc_client_c = rpc_client.clone();
                let met = metrics.clone();
                let tg  = telegram.clone();
                let profit_pct = profit_pct; // ensure it's captured

                tokio::spawn(async move {
                    Self::execute_sell(
                        &mint,
                        rpc_client_c,
                        rpc,
                        bh,
                        wlt,
                        mcm,
                        &reason,
                        profit_pct,
                        met,
                        tg,
                    ).await;
                });
            }
        }
    }

    /// Reads the bonding curve account and parses virtual reserves.
    async fn fetch_price(rpc: &RpcClient, mint_str: &str) -> Option<f64> {
        let mint = Pubkey::from_str(mint_str).ok()?;
        let (bc_pda, _) = derive_bonding_curve(&mint);
        let account = rpc.get_account_with_commitment(
            &bc_pda,
            CommitmentConfig::processed(),
        ).await.ok()?.value?;

        let data = &account.data;
        if data.len() < VIRTUAL_SOL_OFFSET + 8 {
            return None;
        }
        let vtoken = u64::from_le_bytes(data[VIRTUAL_TOKEN_OFFSET..VIRTUAL_TOKEN_OFFSET+8].try_into().ok()?) as f64;
        let vsol   = u64::from_le_bytes(data[VIRTUAL_SOL_OFFSET..VIRTUAL_SOL_OFFSET+8].try_into().ok()?) as f64;

        if vtoken == 0.0 { return None; }
        Some(vsol / vtoken) // price in SOL per token
    }

    async fn execute_sell(
        mint_str: &str,
        rpc_client: Arc<RpcClient>,
        rpc_pool: Arc<RpcPool>,
        blockhash_pool: BlockhashPool,
        wallet: Arc<WalletManager>,
        mc_manager: Arc<MarketCapManager>,
        _reason: &str,
        profit_pct: f64,
        metrics: Option<Arc<crate::metrics::MetricsCollector>>,
        telegram: crate::telegram::ArcTelegram,
    ) {
        let t_start = Instant::now();
        let mint = match Pubkey::from_str(mint_str) {
            Ok(p) => p,
            Err(_) => { error!("Sell: invalid mint {}", mint_str); return; }
        };

        let pump_program = Pubkey::from_str("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P").unwrap();

        let (bonding_curve, _) = derive_bonding_curve(&mint);
        let assoc_bc  = crate::tx_pool::derive_assoc_bonding_curve(&bonding_curve, &mint);
        let buyer_ata = derive_buyer_ata(&wallet.pubkey(), &mint);
        let global_pda = Pubkey::from_str("4wTV81uD9pTAG9XTFVKV3RJ4ZVCNL9b7RPKDjV7h647").unwrap();
        let fee_rec    = Pubkey::from_str("CebN5WGQ4jvEPvsVU4EoHEpgznyZKRL1eovpZTPLbz5").unwrap();

        // Get token balance to sell everything
        let token_amount: u64;
        if rpc_pool.test_mode {
            token_amount = 1_000_000_000; // Mock 1 token
        } else {
            token_amount = rpc_client
                .get_token_account_balance(&buyer_ata).await
                .ok()
                .and_then(|ui| ui.amount.parse::<u64>().ok())
                .unwrap_or(0);
        }

        if token_amount == 0 {
            warn!("Sell: zero balance for {}", mint_str);
            return;
        }

        // Sell discriminator + amount + min_sol_output(0 = accept any)
        let discriminator: [u8; 8] = [0x33, 0xe6, 0x85, 0xa4, 0x01, 0x7f, 0x83, 0xad];
        let mut data = Vec::with_capacity(24);
        data.extend_from_slice(&discriminator);
        data.extend_from_slice(&token_amount.to_le_bytes());
        data.extend_from_slice(&0u64.to_le_bytes()); // min_sol_output = 0

        let sell_ix = Instruction {
            program_id: pump_program,
            accounts: vec![
                AccountMeta::new_readonly(global_pda, false),
                AccountMeta::new(fee_rec, false),
                AccountMeta::new_readonly(mint, false),
                AccountMeta::new(bonding_curve, false),
                AccountMeta::new(assoc_bc, false),
                AccountMeta::new(buyer_ata, false),
                AccountMeta::new(wallet.pubkey(), true),
                AccountMeta::new_readonly(solana_sdk::system_program::id(), false),
                AccountMeta::new_readonly(spl_associated_token_account::id(), false),
                AccountMeta::new_readonly(spl_token::id(), false),
            ],
            data,
        };

        let priority_fee = mc_manager.dynamic_priority_fee().await;
        let compute_price_ix = ComputeBudgetInstruction::set_compute_unit_price(priority_fee);
        let compute_limit_ix = ComputeBudgetInstruction::set_compute_unit_limit(200_000);
        let blockhash = blockhash_pool.get_blockhash().await;

        let msg = match v0::Message::try_compile(
            &wallet.pubkey(),
            &[compute_price_ix, compute_limit_ix, sell_ix],
            &[],
            blockhash,
        ) {
            Ok(m) => m,
            Err(e) => { error!("Sell: msg compile error: {:?}", e); return; }
        };

        let tx = match VersionedTransaction::try_new(VersionedMessage::V0(msg), &[&wallet.keypair]) {
            Ok(t) => t,
            Err(e) => { error!("Sell: sign error: {:?}", e); return; }
        };

        rpc_pool.send_transaction_parallel(&tx).await;
        if let Some(ref m) = metrics { m.inc_sell_success(); }
        info!("SELL EXECUTED ✅ ({}%) mint: {}", (profit_pct * 100.0).round(), mint_str);

        // ── Telegram Alert ───────────────────────────────────────────
        let profit_str = format!("{:.2}%", profit_pct * 100.0);
        let elapsed_str = format!("{}s", t_start.elapsed().as_secs());
        let msg = format!("💰 *SELL EXECUTED*\nMint: `{}`\nProfit: {}\nTime: {}", mint_str, profit_str, elapsed_str);
        tokio::spawn(async move {
            telegram.send_message(&msg).await;
        });
    }
}
