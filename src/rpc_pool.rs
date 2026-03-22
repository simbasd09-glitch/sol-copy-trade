/// rpc_pool.rs — Multi-RPC broadcast with dynamic latency ranking.
/// Pings all endpoints periodically; always uses the fastest first,
/// but broadcasts to ALL in parallel for redundancy.
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::transaction::VersionedTransaction;
use solana_client::rpc_config::RpcSendTransactionConfig;
use solana_transaction_status::UiTransactionEncoding;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};
use solana_sdk::signature::Signature;
use crate::types::TxResult;

#[derive(Clone)]
struct RpcEndpoint {
    client: Arc<RpcClient>,
    url: String,
    latency_ms: u64,
}

pub struct RpcPool {
    endpoints: Arc<RwLock<Vec<RpcEndpoint>>>,
    pub test_mode: bool,
}

impl RpcPool {
    pub fn new(urls: Vec<String>, test_mode: bool) -> Self {
        let endpoints: Vec<RpcEndpoint> = urls
            .into_iter()
            .map(|url| RpcEndpoint {
                client: Arc::new(RpcClient::new(url.clone())),
                url,
                latency_ms: 999,  // start pessimistic — will be ranked after first ping
            })
            .collect();

        let pool = Self {
            endpoints: Arc::new(RwLock::new(endpoints)),
            test_mode,
        };
        if !pool.test_mode {
            pool.start_latency_ranker();
        }
        pool
    }

    /// Background task: pings all RPCs every 5 seconds and sorts by latency.
    fn start_latency_ranker(&self) {
        let eps = self.endpoints.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            loop {
                interval.tick().await;
                let snapshot: Vec<(String, Arc<RpcClient>)> = {
                    let r = eps.read().await;
                    r.iter().map(|ep| (ep.url.clone(), ep.client.clone())).collect()
                };

                let mut results = Vec::new();
                for (url, client) in snapshot {
                    let t = Instant::now();
                    let ok = client.get_slot().await.is_ok();
                    let latency = if ok { t.elapsed().as_millis() as u64 } else { 9999 };
                    results.push((url, latency));
                }

                {
                    let mut w = eps.write().await;
                    for ep in w.iter_mut() {
                        if let Some((_, lat)) = results.iter().find(|(u, _)| *u == ep.url) {
                            ep.latency_ms = *lat;
                        }
                    }
                    w.sort_by_key(|ep| ep.latency_ms);
                    let ranked: Vec<String> = w.iter()
                        .map(|ep| format!("{}ms({})", ep.latency_ms, &ep.url[..ep.url.len().min(30)]))
                        .collect();
                    info!("RPC ranked: {:?}", ranked);
                }
            }
        });
    }

    /// Returns the fastest RPC client (index 0 after sorting).
    pub async fn fastest_client(&self) -> Arc<RpcClient> {
        if self.test_mode {
            return Arc::new(RpcClient::new("http://localhost:8899".to_string()));
        }
        self.endpoints.read().await[0].client.clone()
    }

    /// Serialises transaction once and broadcasts to ALL endpoints in parallel.
    /// `skip_preflight = true`. Returns the first successful signature.
    pub async fn send_transaction_parallel(&self, transaction: &VersionedTransaction) -> Option<Signature> {
        let is_sim = self.test_mode;

        if is_sim {
            tokio::time::sleep(Duration::from_millis(50)).await;
            return Some(Signature::default());
        }

        let config = RpcSendTransactionConfig {
            skip_preflight: true,
            preflight_commitment: None,
            encoding: Some(UiTransactionEncoding::Base64),
            max_retries: Some(0),
            min_context_slot: None,
        };

        let serialized: Vec<u8> = match bincode::serialize(transaction) {
            Ok(b) => b,
            Err(e) => { warn!("TX serialize failed: {}", e); return None; }
        };

        let clients: Vec<(String, Arc<RpcClient>)> = {
            let r = self.endpoints.read().await;
            r.iter().map(|ep| (ep.url.clone(), ep.client.clone())).collect()
        };

        let mut futs = Vec::new();
        for (url, client) in clients {
            let cfg = config.clone();
            let bytes = serialized.clone();
            futs.push(Box::pin(async move {
                let tx: VersionedTransaction = match bincode::deserialize(&bytes) {
                    Ok(t) => t,
                    Err(_) => return (None, url),
                };
                match client.send_transaction_with_config(&tx, cfg).await {
                    Ok(sig) => (Some(sig), url),
                    Err(e) => {
                        let err_str = e.to_string();
                        if err_str.contains("Already processed") || err_str.contains("duplicate") {
                            (None, url) 
                        } else {
                            (None, url)
                        }
                    }
                }
            }));
        }

        // Race all RPCs — first one to return Ok wins.
        let mut remaining = futs;
        while !remaining.is_empty() {
            let (res, _index, next_remaining) = futures::future::select_all(remaining).await;
            if let (Some(sig), _url) = res {
                return Some(sig);
            }
            remaining = next_remaining;
        }

        None
    }

    /// Polls signature status until confirmed or timeout.
    pub async fn wait_for_confirmation(&self, signature: Signature) -> TxResult {
        if self.test_mode {
            return TxResult { signature, confirmed: true, slot: 12345 };
        }

        let rpc = self.fastest_client().await;
        let start = Instant::now();
        let timeout = Duration::from_secs(5);

        while start.elapsed() < timeout {
            match rpc.get_signature_statuses(&[signature]).await {
                Ok(statuses) => {
                    if let Some(Some(status)) = statuses.value.get(0) {
                        if status.err.is_none() {
                            let confirmed = status.confirmations.is_some() || 
                                          status.confirmation_status.as_ref().map(|s| {
                                              use solana_transaction_status::TransactionConfirmationStatus;
                                              matches!(s, TransactionConfirmationStatus::Confirmed | TransactionConfirmationStatus::Finalized)
                                          }).unwrap_or(false);
                            
                            if confirmed {
                                return TxResult {
                                    signature,
                                    confirmed: true,
                                    slot: status.slot,
                                };
                            }
                        } else {
                            return TxResult { signature, confirmed: false, slot: status.slot };
                        }
                    }
                }
                Err(e) => warn!("RPC status check failed: {}", e),
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        TxResult { signature, confirmed: false, slot: 0 }
    }
}
