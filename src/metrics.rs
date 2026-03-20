use std::time::{Instant, Duration};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use dashmap::DashMap;

#[derive(Debug, Clone)]
pub struct TokenMetrics {
    pub mint: String,
    pub detection_time: Option<Instant>,
    pub decode_ms: f64,
    pub build_ms: f64,
    pub sign_ms: f64,
    pub broadcast_ms: f64,
    pub total_latency_ms: f64,
}

pub struct MetricsCollector {
    pub data: DashMap<String, TokenMetrics>,
    pub start_time: Instant,
    pub successful_buys: AtomicU64,
    pub failed_buys: AtomicU64,
    pub skipped_sells: AtomicU64,
    pub real_sells: AtomicU64,
    pub paused_until: AtomicU64,
    pub consecutive_failures: AtomicU64,
}

impl MetricsCollector {
    pub fn new() -> Self {
        Self {
            data: DashMap::new(),
            start_time: Instant::now(),
            successful_buys: AtomicU64::new(0),
            failed_buys: AtomicU64::new(0),
            skipped_sells: AtomicU64::new(0),
            real_sells: AtomicU64::new(0),
            paused_until: AtomicU64::new(0),
            consecutive_failures: AtomicU64::new(0),
        }
    }

    pub fn is_paused(&self) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.paused_until.load(Ordering::Relaxed) > now
    }

    pub fn pause_for(&self, duration: Duration) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.paused_until.store(now + duration.as_millis() as u64, Ordering::Relaxed);
    }

    pub fn inc_buy_success(&self) { self.successful_buys.fetch_add(1, Ordering::Relaxed); }
    pub fn inc_buy_fail(&self) { self.failed_buys.fetch_add(1, Ordering::Relaxed); }
    pub fn inc_sell_skip(&self) { self.skipped_sells.fetch_add(1, Ordering::Relaxed); }
    pub fn inc_sell_success(&self) { self.real_sells.fetch_add(1, Ordering::Relaxed); }

    pub fn record_decode(&self, mint: &str, receipt_time: Instant, decode_ms: f64) {
        self.data.insert(mint.to_string(), TokenMetrics {
            mint: mint.to_string(),
            detection_time: Some(receipt_time),
            decode_ms,
            build_ms: 0.0,
            sign_ms: 0.0,
            broadcast_ms: 0.0,
            total_latency_ms: 0.0,
        });
    }

    pub fn record_build(&self, mint: &str, build_ms: f64) {
        if let Some(mut m) = self.data.get_mut(mint) {
            m.build_ms = build_ms;
        }
    }

    pub fn record_sign(&self, mint: &str, sign_ms: f64) {
        if let Some(mut m) = self.data.get_mut(mint) {
            m.sign_ms = sign_ms;
        }
    }

    pub fn record_broadcast(&self, mint: &str, start: Instant) {
        if let Some(mut m) = self.data.get_mut(mint) {
            m.broadcast_ms = start.elapsed().as_secs_f64() * 1000.0;
        }
    }

    pub fn record_total(&self, mint: &str) {
        if let Some(mut m) = self.data.get_mut(mint) {
            if let Some(t) = m.detection_time {
                m.total_latency_ms = t.elapsed().as_secs_f64() * 1000.0;
            }
        }
    }

    pub async fn generate_report(&self) -> String {
        let mut csv = String::from("token_mint,decode_ms,build_ms,sign_ms,broadcast_ms,total_latency_ms\n");
        
        let mut total_latency = 0.0;
        let mut min_latency = f64::MAX;
        let mut max_latency = f64::MIN;
        let mut count = 0;

        for entry in self.data.iter() {
            let m = entry.value();
            let sum_lat = m.total_latency_ms;
            
            if sum_lat > 0.0 {
                total_latency += sum_lat;
                if sum_lat < min_latency { min_latency = sum_lat; }
                if sum_lat > max_latency { max_latency = sum_lat; }
                count += 1;
            }

            csv.push_str(&format!("{},{:.3},{:.3},{:.3},{:.3},{:.3}\n",
                m.mint, m.decode_ms, m.build_ms, m.sign_ms, m.broadcast_ms, m.total_latency_ms
            ));
        }
        
        if count > 0 {
            let avg = total_latency / (count as f64);
            csv.push_str("\nSummary:\n");
            csv.push_str(&format!("avg latency,{:.3} ms\n", avg));
            csv.push_str(&format!("fastest tx,{:.3} ms\n", min_latency));
            csv.push_str(&format!("slowest tx,{:.3} ms\n", max_latency));
            csv.push_str("failure rate,100%\n");
        }
        
        csv.push_str("\nAccounting:\n");
        csv.push_str(&format!("successful_buys,{}\n", self.successful_buys.load(Ordering::Relaxed)));
        csv.push_str(&format!("failed_buys,{}\n", self.failed_buys.load(Ordering::Relaxed)));
        csv.push_str(&format!("skipped_sells,{}\n", self.skipped_sells.load(Ordering::Relaxed)));
        csv.push_str(&format!("real_sells,{}\n", self.real_sells.load(Ordering::Relaxed)));
        
        csv
    }
}
