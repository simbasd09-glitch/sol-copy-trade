use tracing::Level;
use tracing_subscriber::{fmt, EnvFilter};

pub fn init_logger() {
    let filter = EnvFilter::from_default_env()
        .add_directive(Level::INFO.into());

    let subscriber = fmt::Subscriber::builder()
        .json()
        .with_env_filter(filter)
        .finish();

    tracing::subscriber::set_global_default(subscriber)
        .expect("Failed to set global subscriber");

    tracing::info!("Structured JSON logging initialized");
}
