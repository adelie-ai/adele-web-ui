//! adele-web-ui — axum backend-for-frontend for the mobile web client.
//!
//! NOT designed for public-internet exposure. Bind to loopback or a VPN
//! (Tailscale/WireGuard) interface only. See the README.
//!
//! Front door: the daemon's own `ws-interface` server (`/ws`, `/login`,
//! `/auth/config`), embedded here. Back door: a `client-common::Connector` to
//! the daemon over UDS, driven by a `ForwardingHandler`. Static SPA assets are
//! served once the Leptos app lands (Step 2).

mod auth;
mod config;
mod forward;
mod ws_auth;

use std::sync::Arc;

use anyhow::Context;
use axum::routing::get;
use desktop_assistant_auth_jwt::{default_signing_key_path, ensure_signing_key_at};
use desktop_assistant_client_common::{ConnectionConfig, Connector, TransportMode};
use desktop_assistant_ws::{WsAuthValidator, WsLoginService, WsServeConfig};

use crate::auth::{JwtValidator, PasswordLogin};
use crate::config::BffConfig;
use crate::forward::ForwardingHandler;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let config = BffConfig::load(&BffConfig::default_path())?;
    if !config.enabled {
        tracing::info!(
            "adele-web-ui is disabled in config; exiting. Enable it (via the KCM) to run."
        );
        return Ok(());
    }
    let bind = config.socket_addr()?;

    // Shared HS256 signing key (the daemon's), for browser session tokens.
    let signing_key = ensure_signing_key_at(&default_signing_key_path())
        .context("loading/creating the JWT signing key")?;

    // Back door: a long-lived Connector to the daemon over UDS. The daemon
    // authenticates this process by kernel peer-cred (desktop-assistant#407) —
    // no token is minted; the handshake is tokenless.
    let conn_config = ConnectionConfig {
        transport_mode: TransportMode::Uds,
        socket_path: config.uds_socket.clone(),
        ..ConnectionConfig::default()
    };
    let connector = Connector::connect(&conn_config)
        .await
        .context("connecting to the assistant daemon over UDS")?;
    tracing::info!(daemon = connector.label(), "connected to daemon");
    let handler = Arc::new(ForwardingHandler::new(Arc::new(connector)));

    // Front door: reuse the daemon's ws-interface server (/ws, /login, /auth/config).
    // The browser-token `iss`/`aud` are config-resolved (default: hostname /
    // "<user>.adelie-ai") and shared by the validator + login so they can't drift.
    let issuer = config.issuer();
    let audience = config.audience();
    let validator: Arc<dyn WsAuthValidator> = Arc::new(JwtValidator::new(
        signing_key.clone(),
        issuer.clone(),
        audience.clone(),
    ));
    let login: Option<Arc<dyn WsLoginService>> = config.login_password.clone().map(|password| {
        Arc::new(PasswordLogin::new(
            config.login_username.clone(),
            password,
            signing_key.clone(),
            issuer.clone(),
            audience.clone(),
        )) as Arc<dyn WsLoginService>
    });
    if login.is_none() {
        tracing::warn!("no login_password configured — POST /login will not issue tokens");
    }

    let app = WsServeConfig::new(handler, validator)
        .with_login_service(login)
        .with_allowed_origins(config.allowed_origins.clone())
        .into_router()
        .route("/healthz", get(|| async { "ok" }))
        // Browser WS-auth bridge: relay a `Sec-WebSocket-Protocol` bearer token
        // into the `Authorization` header the embedded ws router validates.
        // No-op for native (header-bearing) clients and non-`/ws` routes.
        .layer(axum::middleware::from_fn(
            ws_auth::inject_bearer_from_subprotocol,
        ));

    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(%bind, "adele-web-ui listening (BFF: /ws, /login, /auth/config, /healthz)");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
