/// telegram.rs — Simple Telegram alerting client.
use serde_json::json;
use std::sync::Arc;
use tracing::{error, info};

pub struct TelegramClient {
    pub token: String,
    pub chat_id: String,
    pub client: reqwest::Client,
}

impl TelegramClient {
    pub fn new(token: String, chat_id: String) -> Self {
        Self {
            token,
            chat_id,
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }

    pub async fn send_message(&self, text: &str) {
        let url = format!("https://api.telegram.org/bot{}/sendMessage", self.token);
        let payload = json!({
            "chat_id": self.chat_id,
            "text": text,
            "parse_mode": "MarkdownV2"
        });

        match self.client.post(url).json(&payload).send().await {
            Ok(resp) => {
                if !resp.status().is_success() {
                    let err_body = resp.text().await.unwrap_or_default();
                    error!("Telegram send failed: {} - {}", text, err_body);
                } else {
                    info!("Telegram alert sent: {}", text);
                }
            }
            Err(e) => error!("Telegram error: {}", e),
        }
    }

    pub fn escape_markdown(text: &str) -> String {
        text.replace("_", "\\_")
            .replace("*", "\\*")
            .replace("[", "\\[")
            .replace("]", "\\]")
            .replace("(", "\\(")
            .replace(")", "\\)")
            .replace("~", "\\~")
            .replace("`", "\\`")
            .replace(">", "\\>")
            .replace("#", "\\#")
            .replace("+", "\\+")
            .replace("-", "\\-")
            .replace("=", "\\=")
            .replace("|", "\\|")
            .replace("{", "\\{")
            .replace("}", "\\}")
            .replace(".", "\\.")
            .replace("!", "\\!")
    }
}

pub type ArcTelegram = Arc<TelegramClient>;
