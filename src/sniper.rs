/// sniper.rs — Production buy execution with real pump.fun PDAs.
/// Uses TxPool for pre-built templates; injects mint + blockhash at runtime.
use crate::rpc_pool::RpcPool;
use crate::blockhash_pool::BlockhashPool;
use crate::speculative::SnipePayload;
use crate::wallet::WalletManager;
use crate::marketcap::MarketCapManager;
use crate::tx_pool::{TxPool, derive_bonding_curve, derive_assoc_bonding_curve, derive_buyer_ata};
use std::sync::Arc;
use std::str::FromStr;
use std::time::{Duration, Instant};
use crate::metrics::MetricsCollector;
use crate::sell::SellMonitor;
use crate::telegram::ArcTelegram;
use solana_sdk::pubkey::Pubkey;
use tracing::{error, info, warn};

pub struct SniperWorker;

impl SniperWorker {
    pub async fn start(
        snipe_rx: async_channel::Receiver<SnipePayload>,
        rpc_pool: Arc<RpcPool>,
        blockhash_pool: BlockhashPool,
        wallet: Arc<WalletManager>,
        mc_manager: Arc<MarketCapManager>,
        tx_pool: Arc<TxPool>,
        mc_threshold: f64,
        buy_amount_usd: f64,
        metrics: Option<Arc<crate::metrics::MetricsCollector>>,
        sell_monitor: Arc<crate::sell::SellMonitor>,
        telegram: crate::telegram::ArcTelegram,
        worker_id: usize,
        shadow_mode: bool,
    ) {
        info!("Sniper worker {} started (shadow_mode={})", worker_id, shadow_mode);

        loop {
            tokio::select! {
                res = snipe_rx.recv() => {
                    match res {
                        Ok(payload) => {
                            if let Some(ref met) = metrics {
                                if met.is_paused() {
                                    warn!("Worker {}: Circuit breaker ACTIVE. Skipping {}", worker_id, payload.mint);
                                    continue;
                                }
                            }
                            let t_receipt = payload.receipt_time;
                            Self::process_snipe(payload, &rpc_pool, &blockhash_pool, &wallet, &mc_manager, &tx_pool, mc_threshold, buy_amount_usd, &metrics, &sell_monitor, &telegram, worker_id, t_receipt, shadow_mode).await;
                        }
                        Err(_) => {
                            info!("Sniper worker {}: snipe channel closed", worker_id);
                            break;
                        }
                    }
                }
            }
        }
    }

    async fn process_snipe(
        payload: SnipePayload,
        rpc_pool: &Arc<RpcPool>,
        blockhash_pool: &BlockhashPool,
        wallet: &Arc<WalletManager>,
        mc_manager: &Arc<MarketCapManager>,
        tx_pool: &Arc<TxPool>,
        mc_threshold: f64,
        buy_amount_usd: f64,
        metrics: &Option<Arc<MetricsCollector>>,
        sell_monitor: &Arc<SellMonitor>,
        telegram: &ArcTelegram,
        worker_id: usize,
        t_receipt: Instant,
        shadow_mode: bool,
    ) {
        if let Some(met) = metrics {
            met.record_decode(&payload.mint, t_receipt, payload.decode_ms);
        }

        if !mc_manager.below_threshold(mc_threshold).await {
            info!("Worker {}: MC gate blocked for {}", worker_id, payload.mint);
            return;
        }

        let mint = match Pubkey::from_str(&payload.mint) {
            Ok(p) => p,
            Err(_) => { warn!("Worker {}: invalid mint {}", worker_id, payload.mint); return; }
        };

        let (bonding_curve, _) = derive_bonding_curve(&mint);
        let assoc_bc = derive_assoc_bonding_curve(&bonding_curve, &mint);
        let buyer_ata = derive_buyer_ata(&wallet.pubkey(), &mint);

        let buy_lamports = mc_manager.usd_to_lamports(buy_amount_usd).await;
        let priority_fee = mc_manager.dynamic_priority_fee().await;
        let blockhash = blockhash_pool.get_blockhash().await;

        let (tx, _b_ms, _s_ms) = match tx_pool.build_buy_tx(
            &mint, &bonding_curve, &assoc_bc, &buyer_ata, &wallet.keypair, blockhash, buy_lamports, priority_fee,
        ).await {
            Some((t, b, s)) => {
                if let Some(ref m) = metrics { 
                    m.record_build(&payload.mint, b);
                    m.record_sign(&payload.mint, s);
                }
                (t, b, s)
            },
            None => { error!("Worker {}: tx build failed for {}", worker_id, payload.mint); return; }
        };

        info!("BUY ATTEMPT → {} (mint={})", "Pending", payload.mint);

        let broadcast_start = Instant::now();
        let sig_opt = rpc_pool.send_transaction_parallel(&tx).await;
        
        if let Some(ref m) = metrics { 
            m.record_broadcast(&payload.mint, broadcast_start);
        }

        let signature = match sig_opt {
            Some(s) => s,
            None => {
                error!("BUY FAILED ❌ (Broadcast error) for {}", payload.mint);
                if let Some(ref m) = metrics { m.inc_buy_fail(); }
                return;
            }
        };

        // Confirmation Guard
        let tx_res = rpc_pool.wait_for_confirmation(signature).await;
        
        if !tx_res.confirmed {
            error!("BUY FAILED ❌ (Confirmation timeout/failure) for {}", payload.mint);
            if let Some(ref m) = metrics { 
                m.inc_buy_fail(); 
                let fails = m.consecutive_failures.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                if fails >= 3 {
                    warn!("CIRCUIT BREAKER: 3+ failures detected. Pausing for 10s.");
                    m.pause_for(Duration::from_secs(10));
                    m.consecutive_failures.store(0, std::sync::atomic::Ordering::Relaxed);
                }
            }
            return;
        }

        let total_ms = t_receipt.elapsed().as_millis();
        info!("BUY CONFIRMED ✅ ({}ms) sig: {}", total_ms, signature);

        if let Some(ref m) = metrics { 
            m.inc_buy_success(); 
            m.consecutive_failures.store(0, std::sync::atomic::Ordering::Relaxed);
            m.record_total(&payload.mint);
        }

        // Shadow Mode Protection
        if shadow_mode {
            let msg = format!("⚡ *SHADOW BUY*\nMint: `{}`\nLatency: {} ms\n\n❌ Expected failure \\(safe\\)", payload.mint, total_ms);
            let tg = telegram.clone();
            tokio::spawn(async move { tg.send_message(&msg).await; });
            return;
        }

        // Add to Sell Monitor
        sell_monitor.add_position(payload.mint.clone(), 0.00000001 /* placeholder */, buy_lamports);
        info!("Position opened for {}", payload.mint);

        // Telegram Success Alert
        let msg = format!("🚀 *BUY EXECUTED*\nMint: `{}`\nTotal Latency: {} ms", payload.mint, total_ms);
        let tg = telegram.clone();
        tokio::spawn(async move { tg.send_message(&msg).await; });

        // Retry burst logic (parallel) - 150ms delay
        let tx_clone = tx.clone();
        let pool_clone = rpc_pool.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            pool_clone.send_transaction_parallel(&tx_clone).await;
        });
    }
}
