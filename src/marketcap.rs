/// marketcap.rs — SOL/USD price cache + slot-delay-based dynamic priority fees.
/// Fee tiers: 30_000 / 45_000 / 60_000 / 80_000 micro-lamports.
/// Upgrades block congestion level from RPC slot lag measurement.
use reqwest::Client;
use solana_client::nonblocking::rpc_client::RpcClient;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

const FEE_TIERS: [u64; 4] = [30_000, 45_000, 60_000, 80_000];

pub struct MarketCapManager {
    pub sol_usd_price: Arc<RwLock<f64>>,
    pub priority_fee:  Arc<RwLock<u64>>,
}

impl MarketCapManager {
    pub fn new(base_priority_fee: u64, rpc: Arc<RpcClient>) -> Self {
        let mgr = Self {
            sol_usd_price: Arc::new(RwLock::new(150.0)),
            priority_fee:  Arc::new(RwLock::new(base_priority_fee)),
        };
        mgr.start_price_updater();
        mgr.start_fee_updater(rpc);
        mgr
    }

    pub async fn sol_price(&self) -> f64 {
        *self.sol_usd_price.read().await
    }

    pub async fn dynamic_priority_fee(&self) -> u64 {
        *self.priority_fee.read().await
    }

    /// Pump.fun virtual-reserve launch MC ≈ 27.959 SOL × price.
    pub async fn below_threshold(&self, threshold_usd: f64) -> bool {
        if std::env::var("RUN_SIMULATION").is_ok() {
            return true;
        }
        let sol_price = *self.sol_usd_price.read().await;
        let launch_mc = 27.959 * sol_price;
        launch_mc < threshold_usd
    }

    pub async fn usd_to_lamports(&self, usd: f64) -> u64 {
        let sol_price = *self.sol_usd_price.read().await;
        ((usd / sol_price) * 1_000_000_000.0) as u64
    }

    fn start_price_updater(&self) {
        if std::env::var("RUN_SIMULATION").is_ok() { return; }
        let sol_price = self.sol_usd_price.clone();
        tokio::spawn(async move {
            let client = Client::new();
            loop {
                let url = "https://api.coingecko.com/api/v3/simple/price?ids=solana&vs_currencies=usd";
                if let Ok(resp) = client.get(url).timeout(std::time::Duration::from_secs(5)).send().await {
                    if let Ok(json) = resp.json::<serde_json::Value>().await {
                        if let Some(price) = json["solana"]["usd"].as_f64() {
                            *sol_price.write().await = price;
                        }
                    }
                }
                tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;
            }
        });
    }

    /// 4-tier Priority Fee System:
    ///   delay 0–1  slots → 30k
    ///   delay 2–3  slots → 45k
    ///   delay 4–6  slots → 60k
    ///   delay 7+   slots → 80k
    fn start_fee_updater(&self, rpc: Arc<RpcClient>) {
        if std::env::var("RUN_SIMULATION").is_ok() { return; }
        let fee_ref = self.priority_fee.clone();
        tokio::spawn(async move {
            let mut last_slot = 0;
            let mut last_ts = std::time::Instant::now();
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));

            loop {
                interval.tick().await;
                if let Ok(slot) = rpc.get_slot().await {
                    let elapsed = last_ts.elapsed().as_secs_f64();
                    let expected_slots = (elapsed * 2.5).round() as u64; // Be slightly aggressive
                    let actual_gap = slot.saturating_sub(last_slot);
                    let delay = expected_slots.saturating_sub(actual_gap);

                    last_slot = slot;
                    last_ts = std::time::Instant::now();

                    let tier = match delay {
                        0..=1 => 0,
                        2..=3 => 1,
                        4..=6 => 2,
                        _     => 3,
                    };
                    
                    let new_fee = FEE_TIERS[tier];
                    *fee_ref.write().await = new_fee;
                    info!(priority_fee = new_fee, delay = delay, "Dynamic fee updated [Tier {}]", tier);
                }
            }
        });
    }
}
