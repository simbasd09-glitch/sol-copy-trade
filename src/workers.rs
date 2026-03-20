/// workers.rs — Full async-channel MPMC worker orchestration.
use std::collections::HashSet;
use std::sync::Arc;

use crate::stream::{StreamWorker, RawEvent};
use crate::decoder::{DecoderWorker, DecodedEvent};
use crate::speculative::{SpeculativeWorker, SnipePayload};
use crate::sniper::SniperWorker;
use crate::sell::SellMonitor;
use crate::dev_monitor::DevMonitor;
use crate::rpc_pool::RpcPool;
use crate::blockhash_pool::BlockhashPool;
use crate::wallet::WalletManager;
use crate::marketcap::MarketCapManager;
use crate::tx_pool::TxPool;
use crate::config::Config;
use crate::metrics::MetricsCollector;
use crate::telegram::{TelegramClient, ArcTelegram};

pub struct SystemWorkers;

impl SystemWorkers {
    pub async fn start_all(
        config: Config,
        rpc_pool: Arc<RpcPool>,
        blockhash_pool: BlockhashPool,
        wallet: Arc<WalletManager>,
        mc_manager: Arc<MarketCapManager>,
        metrics: Arc<MetricsCollector>,
        telegram_client: ArcTelegram,
    ) {
        Self::start_internal(config, rpc_pool, blockhash_pool, wallet, mc_manager, None, Some(metrics), telegram_client).await;
    }

    pub async fn start_test_workers(
        config: Config,
        rpc_pool: Arc<RpcPool>,
        blockhash_pool: BlockhashPool,
        wallet: Arc<WalletManager>,
        mc_manager: Arc<MarketCapManager>,
        decoded_rx: async_channel::Receiver<DecodedEvent>,
        metrics: Arc<MetricsCollector>,
        telegram_client: ArcTelegram,
    ) {
        Self::start_internal(config, rpc_pool, blockhash_pool, wallet, mc_manager, Some(decoded_rx), Some(metrics), telegram_client).await;
    }

    async fn start_internal(
        config: Config,
        rpc_pool: Arc<RpcPool>,
        blockhash_pool: BlockhashPool,
        wallet: Arc<WalletManager>,
        mc_manager: Arc<MarketCapManager>,
        test_rx: Option<async_channel::Receiver<DecodedEvent>>,
        metrics: Option<Arc<MetricsCollector>>,
        telegram_client: ArcTelegram,
    ) {
        let (raw_tx, raw_rx)    = async_channel::bounded::<RawEvent>(4096);
        let (decode_tx, decode_rx) = async_channel::bounded::<DecodedEvent>(4096);
        let (snipe_tx, snipe_rx)   = async_channel::bounded::<SnipePayload>(10000);
        let (alert_tx, alert_rx)   = async_channel::bounded::<String>(256);

        let dev_set: Arc<HashSet<String>> = Arc::new(config.developer_wallet_list.iter().cloned().collect());
        let tx_pool = Arc::new(TxPool::new(config.priority_fee, config.shadow_mode));
        let sell_monitor = Arc::new(SellMonitor::new());

        // ── Stream / Decoder logic (skipped if test_rx is provided) ─────
        if test_rx.is_none() {
            // 1. Stream Workers (N racing connections)
            let ws_urls_from_config = config.ws_endpoints.clone();
            for (i, url) in ws_urls_from_config.into_iter().enumerate() {
                let tx = raw_tx.clone();
                let prog_id = config.pump_fun_program_id.clone();
                tokio::spawn(async move {
                    crate::stream::StreamWorker::start(url, prog_id, tx, i).await;
                });
            }

            // 2. Decoder Workers (4 workers for parallel parsing)
            let target_devs = Arc::new(config.developer_wallet_list.iter().cloned().collect());
            for i in 0..4 {
                let rx = raw_rx.clone();
                let tx = decode_tx.clone();
                let devs = target_devs.clone();
                tokio::spawn(async move {
                    crate::decoder::DecoderWorker::start(rx, devs, tx, i).await;
                });
            }
        }
        drop(decode_tx);

        let final_decode_rx = test_rx.unwrap_or(decode_rx);

        // ── Speculative workers (4) ──────────────────────────────────
        for i in 0..4usize {
            let drx = final_decode_rx.clone();
            let stx = snipe_tx.clone();
            let arx = alert_rx.clone();
            let devs = dev_set.as_ref().clone();
            tokio::spawn(async move { SpeculativeWorker::start(drx, stx, arx, devs, i).await; });
        }
        drop(snipe_tx);

        // ── Sniper workers (16 for stress test) ───────────────────────
        for i in 0..16usize {
            let rx   = snipe_rx.clone();
            let pool = rpc_pool.clone();
            let bh   = blockhash_pool.clone();
            let wlt  = wallet.clone();
            let mcm  = mc_manager.clone();
            let txp  = tx_pool.clone();
            let met  = metrics.clone();
            let sm   = sell_monitor.clone(); // Use the Arc initialized above
            let tg   = telegram_client.clone();
            let thresh = config.marketcap_threshold;
            let buy_usd = config.buy_amount_usd;
            let shadow = config.shadow_mode;
            tokio::spawn(async move {
                SniperWorker::start(rx, pool, bh, wlt, mcm, txp, thresh, buy_usd, met, sm, tg, i, shadow).await;
            });
        }

        // ── Sell monitor workers (4) ──────────────────────────────────
        for i in 0..4usize {
            let sm  = sell_monitor.clone();
            let rpc = rpc_pool.fastest_client().await;
            let pool = rpc_pool.clone();
            let bh   = blockhash_pool.clone();
            let wlt  = wallet.clone();
            let mcm  = mc_manager.clone();
            let met  = metrics.clone();
            let tg   = telegram_client.clone();
            tokio::spawn(async move {
                sm.monitor_loop(rpc, pool, bh, wlt, mcm, met, tg, i).await;
            });
        }

        if !rpc_pool.test_mode {
            let ws_url = config.ws_endpoints.first().cloned()
                .unwrap_or_else(|| "wss://api.mainnet-beta.solana.com".into());
            let devs = dev_set.as_ref().clone();
            let atx = alert_tx;
            tokio::spawn(async move { DevMonitor::start(ws_url, devs, atx, 0).await; });
        }

        tracing::info!("✅ Production workers started.");
    }
}
