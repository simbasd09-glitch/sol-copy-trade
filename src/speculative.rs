/// speculative.rs — Layer 2 Speculative Preparation.
/// Receives decoded events and alert signals from DevMonitor.
/// Prepares complete SnipePayload immediately using data from logs.
/// NO get_transaction RPC call for latency-critical path.
use crate::decoder::DecodedEvent;
use std::collections::HashSet;
use std::time::Instant;
use tracing::{info, warn};

#[derive(Debug, Clone)]
pub struct SnipePayload {
    pub mint: String,
    pub creator: String,
    pub receipt_time: Instant,
    pub decode_ms: f64,
}

pub struct SpeculativeWorker;

impl SpeculativeWorker {
    /// Receives decoded events (already containing mint + creator from logs).
    /// Validates developer whitelist and forwards to sniper queue.
    pub async fn start(
        decoded_rx: async_channel::Receiver<DecodedEvent>,
        snipe_tx: async_channel::Sender<SnipePayload>,
        dev_alert_rx: async_channel::Receiver<String>,
        developer_whitelist: HashSet<String>,
        worker_id: usize,
    ) {
        info!("Speculative worker {} started", worker_id);

        let mut alert_rx_closed = false;
        loop {
            tokio::select! {
                // ── Path A: Full decoded event from decoder  ──────────────
                res = decoded_rx.recv() => {
                    match res {
                        Ok(event) => {
                            let is_sim = std::env::var("RUN_SIMULATION").is_ok() || std::path::Path::new(".simulate").exists();
                            
                            // Developer whitelist check (O(1))
                            if !developer_whitelist.is_empty()
                                && !developer_whitelist.contains(&event.creator)
                                && !is_sim
                            {
                                info!("Speculative {}: discarding token {} - creator not in whitelist", worker_id, event.mint);
                                continue;
                            }

                            if event.mint.is_empty() {
                                continue;
                            }

                            info!(
                                mint = %event.mint,
                                creator = %event.creator,
                                "Speculative {}: passing to sniper", worker_id
                            );

                            let payload = SnipePayload {
                                mint: event.mint,
                                creator: event.creator,
                                receipt_time: event.receipt_time,
                                decode_ms: event.decode_ms,
                            };

                            if let Err(e) = snipe_tx.send(payload).await {
                                warn!("Speculative {}: snipe channel error: {}", worker_id, e);
                                break;
                            }
                        }
                        Err(_) => break, // Channel closed
                    }
                }

                // ── Path B: Pre-emptive alert from DevMonitor ─────────────
                res = dev_alert_rx.recv(), if !alert_rx_closed => {
                    match res {
                        Ok(wallet) => {
                            info!("Speculative {}: pre-emptive alert from wallet {}", worker_id, wallet);
                            // Signal sniper to stand ready — full mint will come from Path A shortly.
                            // In production: pre-build ATA instructions here.
                        }
                        Err(_) => {
                            alert_rx_closed = true;
                            info!("Speculative {}: alert channel closed", worker_id);
                        }
                    }
                }
            }
        }
        info!("Speculative worker {} shut down", worker_id);
    }
}
