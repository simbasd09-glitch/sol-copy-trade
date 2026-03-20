use std::net::SocketAddr;
use tokio::net::TcpListener;
use tokio::io::AsyncWriteExt;
use tracing::info;

/// Minimal HTTP server for Northflank healthchecks.
/// Listens on port 8080 and returns "OK" for any GET request.
pub async fn start_health_server() {
    let addr = SocketAddr::from(([0, 0, 0, 0], 8080));
    let listener = match TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("Failed to bind health server to {}: {}", addr, e);
            return;
        }
    };

    info!("Healthcheck server running on http://{}", addr);

    loop {
        if let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let response = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nContent-Type: text/plain\r\n\r\nOK";
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.flush().await;
            });
        }
    }
}
