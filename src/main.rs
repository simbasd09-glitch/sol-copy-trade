mod config;
mod logger;
mod wallet;
mod rpc_pool;
mod blockhash_pool;
mod stream;
mod decoder;
mod marketcap;
mod speculative;
mod tx_pool;
mod sniper;
mod sell;
mod dev_monitor;
mod workers;
mod types;
mod metrics;
mod simulation;
mod telegram;
mod health;

use std::sync::Arc;
use crate::config::Config;
use crate::rpc_pool::RpcPool;
use crate::blockhash_pool::BlockhashPool;
use crate::wallet::WalletManager;
use crate::marketcap::MarketCapManager;
use crate::tx_pool::TxPool;
use crate::metrics::MetricsCollector;
use tracing::info;

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Load configuration from Environment
    let config = Arc::new(Config::load_from_env());
    
    // 2. Init Logger (Standardized format for Northflank)
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_target(false)
        .init();

    info!("🚀 Starting Production Solana Sniper Bot (Northflank Guard)");

    // 3. Start Health Server (Stateless Container requirement)
    tokio::spawn(health::start_health_server());

    // 4. Initialize Core Engine
    let rpc_pool = Arc::new(RpcPool::new(config.rpc_endpoints.clone(), false));
    let fastest_rpc = rpc_pool.fastest_client().await;
    
    let blockhash_pool = BlockhashPool::new(fastest_rpc.clone()).await;
    let wallet = Arc::new(WalletManager::new(&config.wallet_private_key)?);
    let mc_manager = Arc::new(MarketCapManager::new(config.priority_fee_base, fastest_rpc.clone()));
    let tx_pool = Arc::new(TxPool::new(config.priority_fee_base, config.shadow_mode));
    let metrics = Arc::new(MetricsCollector::new());
    
    let telegram = Arc::new(telegram::TelegramClient::new(
        config.telegram_bot_token.clone(),
        config.telegram_chat_id.clone()
    ));
    
    let sell_monitor = Arc::new(sell::SellMonitor::new());

    info!("Wallet: {}", wallet.pubkey());
    info!("Shadow Mode: {}", config.shadow_mode);

    // 5. Orchestrate Pipeline
    workers::SystemWorkers::start_all(
        (*config).clone(),
        rpc_pool.clone(),
        blockhash_pool,
        wallet,
        mc_manager,
        metrics.clone(),
        telegram,
    ).await;

    // Keep main alive
    tokio::signal::ctrl_c().await?;
    info!("Terminating...");
    Ok(())
}
