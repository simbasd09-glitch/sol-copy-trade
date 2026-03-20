/// dev_monitor.rs — Developer Activity Detection.
/// Subscribes to accountSubscribe for each whitelisted developer wallet.
/// When any activity is detected, starts speculative preparation immediately.
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::collections::HashSet;
use std::time::Duration;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::protocol::Message;
use tracing::{info, warn, error};

pub struct DevMonitor;

impl DevMonitor {
    /// Subscribes to all developer wallets via accountSubscribe.
    /// When a wallet changes state (i.e., sends a tx), sends wallet address
    /// to the speculative prep channel for immediate action.
    pub async fn start(
        ws_url: String,
        developer_wallets: HashSet<String>,
        alert_tx: async_channel::Sender<String>,
        worker_id: usize,
    ) {
        if developer_wallets.is_empty() {
            info!("DevMonitor {}: no wallets to monitor, exiting", worker_id);
            return;
        }

        loop {
            info!("DevMonitor {} connecting...", worker_id);
            match connect_async(&ws_url).await {
                Ok((mut ws, _)) => {
                    info!("DevMonitor {} connected ✓", worker_id);

                    // Subscribe to each developer wallet with accountSubscribe
                    for (i, wallet) in developer_wallets.iter().enumerate() {
                        let sub = json!({
                            "jsonrpc": "2.0",
                            "id": i + 1000,
                            "method": "accountSubscribe",
                            "params": [
                                wallet,
                                {"encoding": "base64", "commitment": "processed"}
                            ]
                        });
                        if ws.send(Message::Text(sub.to_string())).await.is_err() {
                            error!("DevMonitor {} failed to subscribe to {}", worker_id, wallet);
                        }
                    }

                    info!("DevMonitor {}: subscribed to {} wallets", worker_id, developer_wallets.len());

                    while let Some(res) = ws.next().await {
                        match res {
                            Ok(Message::Text(text)) => {
                                if text.contains("\"accountNotification\"") {
                                    // Extract wallet from notification (via subscription id lookup or context)
                                    // For now: broadcast generic "activity" alert to speculative pipeline
                                    info!("DevMonitor {}: developer wallet activity detected!", worker_id);
                                    for wallet in &developer_wallets {
                                        if text.contains(wallet.as_str()) {
                                            let _ = alert_tx.try_send(wallet.clone());
                                            break;
                                        }
                                    }
                                }
                            }
                            Ok(Message::Close(_)) => {
                                warn!("DevMonitor {} connection closed", worker_id);
                                break;
                            }
                            Err(e) => {
                                error!("DevMonitor {} error: {}", worker_id, e);
                                break;
                            }
                            _ => {}
                        }
                    }
                }
                Err(e) => error!("DevMonitor {} connect failed: {}", worker_id, e),
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }
}
