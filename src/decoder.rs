/// decoder.rs — Zero-RPC mint/creator extraction from pump.fun logsNotification.
/// Three detection strategies in order of priority:
///   S1 — JSON log entry {"mint":"...", "user":"..."}
///   S2 — key=value plain text  mint=... user=...
///   S3 — instruction accounts heuristic (accounts[0]=creator, accounts[1]=mint)
use serde::Deserialize;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;
use crate::stream::RawEvent;
use tracing::{debug, info, warn};

// ─── JSON structures ──────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct LogsNotification {
    params: LogsParams,
}

#[derive(Debug, Deserialize)]
struct LogsParams {
    result: LogsResult,
}

#[derive(Debug, Deserialize)]
struct LogsResult {
    value: LogsValue,
}

#[derive(Debug, Deserialize)]
struct LogsValue {
    signature: String,
    err: Option<serde_json::Value>,
    logs: Vec<String>,
}

// ─── Decoded event ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct DecodedEvent {
    pub signature: String,
    pub mint: String,
    pub creator: String,
    pub logs: Vec<String>,
    pub receipt_time: Instant,
    pub decode_ms: f64,
}

// ─── Worker ───────────────────────────────────────────────────────────────────

pub struct DecoderWorker;

impl DecoderWorker {
    pub async fn start(
        raw_rx: async_channel::Receiver<RawEvent>,
        target_developers: Arc<HashSet<String>>,
        decoded_tx: async_channel::Sender<DecodedEvent>,
        worker_id: usize,
    ) {
        info!("Decoder worker {} started", worker_id);

        while let Ok(raw_event) = raw_rx.recv().await {
            let RawEvent { text: raw_msg, received_at: receipt_time } = raw_event;
            
            // Parse the full notification JSON once
            let root: serde_json::Value = match serde_json::from_str(&raw_msg) {
                Ok(v) => v,
                Err(_) => continue,
            };

            // Skip non-notification messages
            if root.get("method").and_then(|m| m.as_str()) != Some("logsNotification") {
                continue;
            }

            let value = &root["params"]["result"]["value"];
            if value.is_null() { continue; }

            // Skip failed transactions
            if !value["err"].is_null() { continue; }

            let sig = match value["signature"].as_str() {
                Some(s) => s.to_string(),
                None => continue,
            };

            let logs: Vec<String> = value["logs"]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
                .unwrap_or_default();

            // Must contain a Create instruction
            let is_create = logs.iter().any(|l| {
                l.contains("Instruction: Create")
                    || l.contains("Program log: Instruction: Create")
            });
            if !is_create { continue; }

            // ── Three-strategy extraction ────────────────────────────────────
            let (mint, creator) = extract_mint_creator(&logs, &root);

            if mint.is_empty() {
                debug!("Decoder {}: Create seen but mint not found in {}", worker_id, sig);
                continue;
            }

            // ── Developer whitelist ───────────────────────────────────────────
            if !target_developers.is_empty() && !target_developers.contains(&creator) {
                debug!("Decoder {}: creator {} not whitelisted", worker_id, creator);
                continue;
            }

            info!(
                sig = %sig, mint = %mint, creator = %creator,
                "Decoder {}: event decoded", worker_id
            );

            let decode_ms = receipt_time.elapsed().as_secs_f64() * 1000.0;
            let event = DecodedEvent { signature: sig, mint, creator, logs, receipt_time, decode_ms };
            if let Err(e) = decoded_tx.try_send(event) {
                warn!("Decoder {}: channel error: {}", worker_id, e);
            }
        }
    }
}

// ─── Three-strategy mint/creator extractor ────────────────────────────────────

fn extract_mint_creator(logs: &[String], root: &serde_json::Value) -> (String, String) {
    let mut mint    = String::new();
    let mut creator = String::new();

    // ── Strategy 1: JSON encoded "Program log: {...}" line ───────────────────
    for line in logs {
        if line.contains("\"mint\"") && line.contains("\"user\"") {
            if let Some(start) = line.find('{') {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&line[start..]) {
                    if let Some(m) = val["mint"].as_str().or_else(|| val["data"]["mint"].as_str()) {
                        mint = m.to_string();
                    }
                    if let Some(u) = val["user"].as_str().or_else(|| val["data"]["user"].as_str()) {
                        creator = u.to_string();
                    }
                    if !mint.is_empty() { return (mint, creator); }
                }
            }
        }

        // ── Strategy 2: key=value plain text log ─────────────────────────────
        if line.contains("mint=") {
            for part in line.split_whitespace() {
                if let Some(v) = part.strip_prefix("mint=") {
                    mint = v.trim_matches(',').to_string();
                }
                if let Some(v) = part.strip_prefix("user=") {
                    creator = v.trim_matches(',').to_string();
                }
            }
            if !mint.is_empty() { return (mint, creator); }
        }
    }

    // ── Strategy 3: Instruction accounts heuristic ──────────────────────────
    // Parse logsNotification → transaction.message.accountKeys[] if embedded
    // For logsSubscribe only, the field lives in:
    //   root["params"]["result"]["value"]["transaction"]["message"]["accountKeys"]
    // When using logsSubscribe (not transactionSubscribe), accounts are not sent.
    // However, some RPC providers embed them. Also check the instructions array.
    if mint.is_empty() {
        let pump_prog = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";
        let tx_msg = &root["params"]["result"]["value"]["transaction"]["message"];
        let account_keys = tx_msg["accountKeys"].as_array();
        let instructions  = tx_msg["instructions"].as_array();

        if let (Some(keys), Some(ixs)) = (account_keys, instructions) {
            for ix in ixs {
                // Find the pump.fun instruction
                let prog_idx = ix["programIdIndex"].as_u64().unwrap_or(999) as usize;
                let prog_key = keys.get(prog_idx).and_then(|k| k.as_str()).unwrap_or("");
                if prog_key != pump_prog { continue; }

                // accounts[0] = creator, accounts[1] = mint (pump.fun Create layout)
                if let Some(accs) = ix["accounts"].as_array() {
                    let get_key = |idx: u64| {
                        keys.get(idx as usize).and_then(|k| k.as_str()).unwrap_or("").to_string()
                    };
                    if let Some(i0) = accs.get(0).and_then(|v| v.as_u64()) {
                        creator = get_key(i0);
                    }
                    if let Some(i1) = accs.get(1).and_then(|v| v.as_u64()) {
                        mint = get_key(i1);
                    }
                }
                break;
            }
        }
    }

    (mint, creator)
}
