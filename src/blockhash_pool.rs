use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::hash::Hash;
use std::sync::Arc;
use tokio::sync::RwLock;
use std::time::Duration;

#[derive(Clone)]
pub struct BlockhashPool {
    latest_blockhash: Arc<RwLock<Hash>>,
    rpc_client: Arc<RpcClient>,
}

impl BlockhashPool {
    pub async fn new(rpc_client: Arc<RpcClient>) -> Self {
        let pool = Self {
            latest_blockhash: Arc::new(RwLock::new(Hash::default())),
            rpc_client,
        };

        // Don't block here. Let the updater fetch it.
        pool.start_refresh_task();
        pool
    }

    pub async fn get_blockhash(&self) -> Hash {
        *self.latest_blockhash.read().await
    }

    fn start_refresh_task(&self) {
        // Spawn background refresh task (skipped in simulation)
        if std::env::var("RUN_SIMULATION").is_err() {
            let blockhash_ref = self.latest_blockhash.clone();
            let client = self.rpc_client.clone();

            tokio::spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_millis(400));
                loop {
                    interval.tick().await;
                    match client.get_latest_blockhash().await {
                        Ok(new_hash) => {
                            let mut bh = blockhash_ref.write().await;
                            *bh = new_hash;
                        }
                        Err(e) => {
                            tracing::warn!("Failed to fetch latest blockhash: {:?}", e);
                        }
                    }
                }
            });
        }
    }
}
