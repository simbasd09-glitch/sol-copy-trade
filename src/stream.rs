/// stream.rs — Multi-WebSocket Stream Racing.
/// Spawns N parallel WebSocket connections; whichever gets a message first
/// wins and pushes to the shared decoder queue. Self-healing on disconnect.
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::time::Duration;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use tracing::{error, info, warn};
use std::time::Instant;

pub struct RawEvent {
    pub text: String,
    pub received_at: Instant,
}

pub struct StreamWorker;

impl StreamWorker {
    /// Start a persistent, auto-reconnecting logsSubscribe WebSocket for
    /// pump.fun. Multiple instances of this race each other — first wins.
    pub async fn start(
        ws_url: String,
        program_id: String,
        event_sender: async_channel::Sender<RawEvent>,
        worker_id: usize,
    ) {
        loop {
            info!("Stream worker {} connecting → {}", worker_id, ws_url);
            match connect_async(&ws_url).await {
                Ok((mut ws_stream, _)) => {
                    info!("Stream worker {} connected ✓", worker_id);

                    let sub_msg = json!({
                        "jsonrpc": "2.0",
                        "id": worker_id,
                        "method": "logsSubscribe",
                        "params": [
                            {"mentions": [program_id]},
                            {"commitment": "processed"}
                        ]
                    });

                    if ws_stream.send(Message::Text(sub_msg.to_string())).await.is_err() {
                        error!("Stream worker {} send subscribe failed", worker_id);
                        tokio::time::sleep(Duration::from_secs(2)).await;
                        continue;
                    }

                    let mut ping_interval = tokio::time::interval(Duration::from_secs(15));
                    
                    loop {
                        tokio::select! {
                            _ = ping_interval.tick() => {
                                if ws_stream.send(Message::Ping(vec![])).await.is_err() {
                                    break;
                                }
                            }
                            msg_result = ws_stream.next() => {
                                match msg_result {
                                    Some(Ok(Message::Text(text))) => {
                                        if text.contains("\"logsNotification\"") {
                                            let event = RawEvent { text, received_at: Instant::now() };
                                            if event_sender.try_send(event).is_err() {
                                                warn!("Stream worker {}: queue full", worker_id);
                                            }
                                        }
                                    }
                                    Some(Ok(Message::Pong(_))) => {}
                                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                                    _ => {}
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    error!("Stream worker {} connect failed: {}", worker_id, e);
                }
            }

            warn!("Stream worker {} reconnecting in 2s...", worker_id);
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }
}
