use anyhow::{Context, Result};
use lightning_pi::{
    server::{self, Config, LexeReceiver},
    store::ReplayStore,
    wallet::load_wallet,
};
use std::{path::Path, sync::Arc};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let origin = std::env::var("PUBLIC_ORIGIN").context("PUBLIC_ORIGIN is required")?;
    let config = Config::new(
        origin,
        std::env::var("PRICE_MSAT")
            .unwrap_or("100000".into())
            .parse()?,
        std::env::var("MAX_DIGITS")
            .unwrap_or("10000".into())
            .parse()?,
    )?;
    let db = std::env::var("REPLAY_DB").unwrap_or("data/replay.sqlite".into());
    if let Some(parent) = Path::new(&db)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    let store = Arc::new(ReplayStore::open(Path::new(&db))?);
    let wallet = load_wallet()?;
    let node = wallet.node_info().await?;
    let receiver = Arc::new(LexeReceiver {
        wallet,
        pay_to: node.node_pk.to_string(),
    });
    // The facilitator is loopback-only; Fly exposes only port 8080.
    let facilitator = tokio::net::TcpListener::bind("127.0.0.1:8081").await?;
    let app = server::app(config, receiver, "http://127.0.0.1:8081/settle".into());
    tokio::spawn(async move {
        if let Err(error) = axum::serve(facilitator, server::facilitator(store)).await {
            tracing::error!(%error,"facilitator stopped");
            std::process::exit(1);
        }
    });
    let addr = std::env::var("LISTEN_ADDR").unwrap_or("0.0.0.0:8080".into());
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!(%addr,"Lightning pi listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
