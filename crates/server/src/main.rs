//! adele-web-ui — axum backend-for-frontend for the mobile web client.
//!
//! NOT designed for public-internet exposure. Bind to loopback or a VPN
//! (Tailscale/WireGuard) interface only. See the README.
//!
//! This is an early scaffold: it currently serves a health endpoint so the
//! service can be deployed and toggled while the real front door (embedded
//! `ws-interface`) and the UDS forwarding back door are built out.

use std::net::SocketAddr;

use axum::{Router, routing::get};

/// Default listen port for the web UI service.
const DEFAULT_PORT: u16 = 9379;
/// Default bind address — loopback. Override to a VPN interface, never a public address.
const DEFAULT_BIND: &str = "127.0.0.1";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let bind: SocketAddr = format!("{DEFAULT_BIND}:{DEFAULT_PORT}").parse()?;

    let app = Router::new().route("/healthz", get(|| async { "ok" }));

    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(%bind, "adele-web-ui listening (scaffold)");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
