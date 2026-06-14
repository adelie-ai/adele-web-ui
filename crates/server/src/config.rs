//! BFF configuration (TOML), with defaults. Also written by the KCM.
//!
//! NOT for the public internet — `bind_address` defaults to loopback; reach the
//! service over Tailscale/WireGuard and, if you change the bind, point it at the
//! tunnel interface. See the README.

use std::net::SocketAddr;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Default listen port for the web UI service.
pub const DEFAULT_PORT: u16 = 9379;
/// Default bind address — loopback. Override to a VPN interface, never public.
pub const DEFAULT_BIND: &str = "127.0.0.1";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BffConfig {
    /// Whether the service runs at all (the KCM toggle). Default off.
    pub enabled: bool,
    /// Interface to bind. Loopback by default; set to your Tailscale/WireGuard
    /// address to reach it from a phone. Never a public address.
    pub bind_address: String,
    /// Listen port (default 9379).
    pub port: u16,
    /// Browser `Origin` allowlist. Empty rejects browser clients, so this must
    /// be set for the SPA to connect (seeded from the bind/host).
    pub allowed_origins: Vec<String>,
    /// Username accepted at `POST /login`.
    pub login_username: String,
    /// Static login password. `None` disables password login (`/login` returns
    /// no token); PAM/system auth is a follow-up.
    pub login_password: Option<String>,
    /// UDS socket to the daemon. `None` resolves to the platform default.
    pub uds_socket: Option<PathBuf>,
    /// Local JWT minter socket — the BFF mints a fresh daemon token per connect
    /// (peer-UID authenticated), e.g. `$XDG_RUNTIME_DIR/adelie/mint.sock`. In
    /// practice this is **required**: the daemon's UDS front door still demands a
    /// bearer token, so with `None` the Connector fails to authenticate ("no JWT
    /// provided") unless a token is supplied another way. Set it to the daemon's
    /// minter socket. (Verified live 2026-06-14.)
    pub minter_socket: Option<PathBuf>,
}

impl Default for BffConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bind_address: DEFAULT_BIND.to_string(),
            port: DEFAULT_PORT,
            allowed_origins: Vec::new(),
            login_username: "adele".to_string(),
            login_password: None,
            uds_socket: None,
            minter_socket: None,
        }
    }
}

impl BffConfig {
    /// Resolve the bind `SocketAddr` from `bind_address` + `port`.
    pub fn socket_addr(&self) -> anyhow::Result<SocketAddr> {
        Ok(format!("{}:{}", self.bind_address, self.port).parse()?)
    }

    /// Load from `path`, falling back to defaults when the file is absent.
    /// A present-but-malformed file is an error (don't silently run on defaults
    /// when the operator clearly intended a config).
    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(toml::from_str(&text)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Default config path: `$XDG_CONFIG_HOME/adele-web-ui/config.toml`
    /// (or `~/.config/adele-web-ui/config.toml`).
    pub fn default_path() -> PathBuf {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("adele-web-ui").join("config.toml")
    }
}
