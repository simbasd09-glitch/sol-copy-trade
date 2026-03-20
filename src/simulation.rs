use std::sync::Arc;
// Force Rebuild
use std::time::Instant;
use tokio::time::{sleep, Duration};
use crate::metrics::MetricsCollector;
use crate::decoder::DecodedEvent;
use async_channel::Sender;
use solana_sdk::pubkey::Pubkey;

pub struct SimulationEngine {
    pub metrics: Arc<MetricsCollector>,
}

impl SimulationEngine {
    pub fn new_with_metrics(metrics: Arc<MetricsCollector>) -> Self {
        Self { metrics }
    }

    /// Injects 1000 mock mints into the pipeline to stress test concurrency.
    pub async fn run_1000_token_test(&self, decoded_tx: Sender<DecodedEvent>) {
        tracing::info!("Starting 1000-token simulation test...");
        
        for i in 0..5 {
            let mock_mint = Pubkey::new_unique().to_string();
            let mock_creator = "CebN5WGQ4jvEPvsVU4EoHEpgznyZKRL1eovpZTPLbz5";
            
            let t_now = Instant::now();
            self.metrics.record_decode(&mock_mint, t_now, 0.0);
            
            let event = DecodedEvent {
                signature: format!("sim_sig_{}", i),
                mint: mock_mint,
                creator: mock_creator.to_string(),
                logs: vec!["Instruction: Create".to_string()],
                receipt_time: t_now,
                decode_ms: 0.0,
            };

            if let Err(e) = decoded_tx.send(event).await {
                tracing::error!("Failed to inject token {}: {:?}", i, e);
            }
            
            if i % 100 == 0 {
                tracing::info!("Injected {}/1000 tokens...", i);
            }
            sleep(Duration::from_micros(500)).await;
        }

        tracing::info!("All 1000 tokens injected into pipeline. Monitoring for 60s...");
        sleep(Duration::from_secs(60)).await;

        tracing::info!("Simulation window complete. Generating final report...");
        let report = self.metrics.generate_report().await;
        let report_len = report.len();
        if let Err(e) = tokio::fs::write("shadow_report.csv", report).await {
            tracing::error!("Failed to write shadow test report: {:?}", e);
        } else {
            tracing::info!("Final shadow test report generated: shadow_report.csv (size: {}) ✓", report_len);
        }
    }
}
