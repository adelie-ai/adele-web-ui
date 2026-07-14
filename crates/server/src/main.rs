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
use tower_http::services::{ServeDir, ServeFile};

use crate::auth::{JwtValidator, PasswordLogin};
use crate::config::{BffConfig, DaemonTransport};
use crate::forward::ForwardingHandler;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let mut config = BffConfig::load(&BffConfig::default_path())?;
    // Env (`ADELE_WEB_UI_*`) overlays the TOML so a container / systemd unit can
    // configure the service with no config file. Env wins over file.
    config.apply_env_overrides();
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

    // Back door: a long-lived Connector to the daemon. Two ways in:
    //  * UDS (default) — a co-located daemon authenticates this process by kernel
    //    peer-cred (desktop-assistant#407); no token is minted (tokenless).
    //  * WS — a remote daemon (e.g. on k8s); auth is the daemon's `/login`
    //    password exchange (or a pre-minted `daemon_ws_jwt`). Mirrors the tui/gtk
    //    `ConnectionConfig`. `tls_ca_cert` (default) is unused for `ws://`.
    let (conn_config, back_door) = match config.daemon_transport {
        DaemonTransport::Uds => (
            ConnectionConfig {
                transport_mode: TransportMode::Uds,
                socket_path: config.uds_socket.clone(),
                ..ConnectionConfig::default()
            },
            "UDS".to_string(),
        ),
        DaemonTransport::Ws => {
            let ws_url = config.daemon_ws_url.clone().context(
                "daemon_transport = ws requires daemon_ws_url (ADELE_WEB_UI_DAEMON_WS_URL)",
            )?;
            let back_door = format!("WS {ws_url}");
            (
                ConnectionConfig {
                    transport_mode: TransportMode::Ws,
                    ws_url,
                    ws_login_username: config.daemon_ws_username.clone(),
                    ws_login_password: config.daemon_ws_password.clone(),
                    ws_jwt: config.daemon_ws_jwt.clone(),
                    // No custom CA: a plain `ws://` back door needs none, and for
                    // `wss://` reqwest/tungstenite fall back to the system roots.
                    // The default `Some(<XDG>/…/ca.pem)` would force reading a
                    // daemon CA file that doesn't exist in the container (and the
                    // in-cluster daemon runs `ws://`, TLS off). A self-signed
                    // `wss://` CA would be a follow-up env var.
                    tls_ca_cert: None,
                    ..ConnectionConfig::default()
                },
                back_door,
            )
        }
    };
    let connector = Connector::connect(&conn_config)
        .await
        .with_context(|| format!("connecting to the assistant daemon over {back_door}"))?;
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

    let mut app = WsServeConfig::new(handler, validator)
        .with_login_service(login)
        .with_allowed_origins(config.allowed_origins.clone())
        .into_router()
        .route("/healthz", get(|| async { "ok" }));

    // Static SPA at `/`, mounted as the router fallback so it never shadows the
    // API routes (`/ws`, `/login`, `/auth/config`, `/healthz`). Unknown paths
    // fall back to `index.html` for client-side routing. If the dir is absent we
    // log and skip — the BFF still serves its API (do NOT crash).
    let web_dir = config.web_dir();
    if web_dir.is_dir() {
        let serve_dir =
            ServeDir::new(&web_dir).fallback(ServeFile::new(web_dir.join("index.html")));
        app = app.fallback_service(serve_dir);
        tracing::info!(dir = %web_dir.display(), "serving SPA static assets at /");
    } else {
        tracing::warn!(
            dir = %web_dir.display(),
            "SPA asset dir not found — serving API only (no static UI at /)"
        );
    }

    // Browser WS-auth bridge: relay a `Sec-WebSocket-Protocol` bearer token into
    // the `Authorization` header the embedded ws router validates. No-op for
    // native (header-bearing) clients and non-`/ws` routes.
    let app = app.layer(axum::middleware::from_fn(
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
