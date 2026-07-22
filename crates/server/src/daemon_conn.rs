//! The `client-common` connection config for the BFF's back-door link to the
//! daemon.
//!
//! Extracted from `main` so the connection settings are unit-testable without
//! standing up a daemon. See [`build_daemon_connection_config`] for the #557
//! client-context posture.

use anyhow::Context;
use desktop_assistant_client_common::{ConnectionConfig, TransportMode};

use crate::config::{BffConfig, DaemonTransport};

/// Build the `client-common` [`ConnectionConfig`] for the BFF's back-door
/// connection to the daemon, plus a short human label for logs.
///
/// Two ways in, mirroring the tui/gtk `ConnectionConfig`:
///  * UDS (default) — a co-located daemon authenticates this process by kernel
///    peer-cred (desktop-assistant#407); no token is minted (tokenless).
///  * WS — a remote daemon (e.g. on k8s); auth is the daemon's `/login`
///    password exchange (or a pre-minted `daemon_ws_jwt`). `tls_ca_cert` is
///    left unset: the in-cluster daemon runs plain `ws://` (TLS off), and the
///    default `Some(<XDG>/…/ca.pem)` would force reading a daemon CA file that
///    doesn't exist in the container; for `wss://`, reqwest/tungstenite fall
///    back to the system roots.
///
/// # Client context (#557)
///
/// `share_client_context` is forced **off** on both transports. It gates
/// client-common's native `resolve_client_context`, which reads THIS process's
/// environment — home dir, username, hostname, timezone, OS. For the BFF that
/// is the *server's* machine, not the browser user's, so sharing it would put
/// false personal and device facts in the daemon's system prompt. A browser
/// user's context is limited to what a browser can actually know — timezone
/// (`Intl.DateTimeFormat`) and a coarse platform — resolved in the wasm client
/// and attached separately; the BFF never uses the local-environment resolver.
pub fn build_daemon_connection_config(
    config: &BffConfig,
) -> anyhow::Result<(ConnectionConfig, String)> {
    match config.daemon_transport {
        DaemonTransport::Uds => Ok((
            ConnectionConfig {
                transport_mode: TransportMode::Uds,
                socket_path: config.uds_socket.clone(),
                share_client_context: false,
                ..ConnectionConfig::default()
            },
            "UDS".to_string(),
        )),
        DaemonTransport::Ws => {
            let ws_url = config.daemon_ws_url.clone().context(
                "daemon_transport = ws requires daemon_ws_url (ADELE_WEB_UI_DAEMON_WS_URL)",
            )?;
            let back_door = format!("WS {ws_url}");
            Ok((
                ConnectionConfig {
                    transport_mode: TransportMode::Ws,
                    ws_url,
                    ws_login_username: config.daemon_ws_username.clone(),
                    ws_login_password: config.daemon_ws_password.clone(),
                    ws_jwt: config.daemon_ws_jwt.clone(),
                    tls_ca_cert: None,
                    share_client_context: false,
                    ..ConnectionConfig::default()
                },
                back_door,
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn ws_config() -> BffConfig {
        BffConfig {
            daemon_transport: DaemonTransport::Ws,
            daemon_ws_url: Some("ws://adele-daemon:11339/ws".to_string()),
            daemon_ws_username: Some("adele".to_string()),
            daemon_ws_password: Some("s3cret".to_string()),
            ..BffConfig::default()
        }
    }

    #[test]
    fn uds_back_door_never_shares_the_bff_server_environment() {
        // #557: the BFF runs on a server that is the WRONG machine for a browser
        // user. `share_client_context` gates client-common's native resolver,
        // which reads THIS host's home dir / username / hostname / timezone / OS
        // — none of which are the browser user's. It must be OFF so the BFF never
        // sends false personal or device facts to the daemon.
        let (conn, label) =
            build_daemon_connection_config(&BffConfig::default()).expect("uds config builds");
        assert_eq!(conn.transport_mode, TransportMode::Uds);
        assert!(
            !conn.share_client_context,
            "BFF must not share its own server environment as client context"
        );
        assert_eq!(label, "UDS");
    }

    #[test]
    fn ws_back_door_never_shares_the_bff_server_environment() {
        let (conn, label) = build_daemon_connection_config(&ws_config()).expect("ws config builds");
        assert_eq!(conn.transport_mode, TransportMode::Ws);
        assert!(
            !conn.share_client_context,
            "BFF must not share its own server environment as client context"
        );
        assert_eq!(conn.ws_url, "ws://adele-daemon:11339/ws");
        assert_eq!(conn.ws_login_username.as_deref(), Some("adele"));
        assert_eq!(conn.ws_login_password.as_deref(), Some("s3cret"));
        assert!(label.starts_with("WS "));
    }

    #[test]
    fn uds_back_door_uses_the_configured_socket_path() {
        let cfg = BffConfig {
            uds_socket: Some(PathBuf::from("/run/adelie/sock")),
            ..BffConfig::default()
        };
        let (conn, _) = build_daemon_connection_config(&cfg).expect("builds");
        assert_eq!(conn.socket_path, Some(PathBuf::from("/run/adelie/sock")));
    }

    #[test]
    fn ws_back_door_omits_the_custom_ca() {
        // The in-cluster daemon runs plain ws:// (TLS off); a default CA path
        // would force reading a file that isn't present in the container.
        let (conn, _) = build_daemon_connection_config(&ws_config()).expect("builds");
        assert!(conn.tls_ca_cert.is_none());
    }

    #[test]
    fn ws_back_door_requires_a_url() {
        let cfg = BffConfig {
            daemon_transport: DaemonTransport::Ws,
            daemon_ws_url: None,
            ..BffConfig::default()
        };
        let err =
            build_daemon_connection_config(&cfg).expect_err("ws without a url must be an error");
        assert!(
            err.to_string().contains("daemon_ws_url"),
            "error should name the missing url, got: {err}"
        );
    }
}
