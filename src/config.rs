use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
pub struct Config {
    pub wallet_private_key: String,
    pub rpc_endpoints: Vec<String>,
    pub ws_endpoints: Vec<String>,
    pub pump_fun_program_id: String,
    pub developer_wallet_list: Vec<String>,
    pub buy_amount_usd: f64,
    pub marketcap_threshold: f64,
    pub priority_fee_base: u64,
    pub max_concurrent_trades: usize,
    pub shadow_mode: bool,
    pub telegram_bot_token: String,
    pub telegram_chat_id: String,
}

impl Config {
    pub fn load_from_env() -> Self {
        dotenvy::dotenv().ok();

        let private_key = std::env::var("PRIVATE_KEY")
            .expect("PRIVATE_KEY must be set");

        let mut rpc_endpoints = Vec::new();
        for i in 1..=12 {
            if let Ok(url) = std::env::var(format!("RPC_URL_{}", i)) {
                rpc_endpoints.push(url);
            }
        }
        if rpc_endpoints.is_empty() {
            // Fallback for local testing if not using indexed vars
            if let Ok(urls) = std::env::var("RPC_URLS") {
                rpc_endpoints = urls.split(',').map(|s| s.trim().to_string()).collect();
            }
        }

        let mut ws_endpoints = Vec::new();
        for i in 1..=4 {
            if let Ok(url) = std::env::var(format!("WS_URL_{}", i)) {
                ws_endpoints.push(url);
            }
        }

        let pump_id = std::env::var("PUMP_FUN_PROGRAM_ID")
            .unwrap_or_else(|_| "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P".to_string());

        let dev_list = std::env::var("DEVELOPER_WALLETS")
            .unwrap_or_default()
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|s| s.trim().to_string())
            .collect();

        Config {
            wallet_private_key: private_key,
            rpc_endpoints,
            ws_endpoints,
            pump_fun_program_id: pump_id,
            developer_wallet_list: dev_list,
            buy_amount_usd: std::env::var("BUY_AMOUNT_USD")
                .unwrap_or_else(|_| "1.0".to_string())
                .parse()
                .unwrap_or(1.0),
            marketcap_threshold: std::env::var("MARKETCAP_THRESHOLD")
                .unwrap_or_else(|_| "50000".to_string())
                .parse()
                .unwrap_or(50000.0),
            priority_fee_base: std::env::var("PRIORITY_FEE_BASE")
                .unwrap_or_else(|_| "30000".to_string())
                .parse()
                .unwrap_or(30000),
            max_concurrent_trades: std::env::var("MAX_CONCURRENT_TRADES")
                .unwrap_or_else(|_| "4".to_string())
                .parse()
                .unwrap_or(4),
            shadow_mode: std::env::var("SHADOW_MODE")
                .unwrap_or_else(|_| "true".to_string())
                .to_lowercase() == "true",
            telegram_bot_token: std::env::var("TELEGRAM_TOKEN").unwrap_or_default(),
            telegram_chat_id: std::env::var("CHAT_ID").unwrap_or_default(),
        }
    }
}
