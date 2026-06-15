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
    /// JWT `iss` for the browser session tokens the BFF issues + validates.
    /// `None`/empty ⇒ the local hostname. Mirrors the daemon's
    /// `[ws_auth.hs256].issuer`; the BFF validates its own tokens, so this just
    /// has to be self-consistent (issue == validate, which it is by construction).
    pub issuer: Option<String>,
    /// JWT `aud` for browser session tokens. `None`/empty ⇒ `"<user>.adelie-ai"`
    /// (mirrors the daemon's `[ws_auth.hs256].audience`).
    pub audience: Option<String>,
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
            issuer: None,
            audience: None,
        }
    }
}

/// Best-effort local hostname for the default token `iss`. Dependency-free
/// (kernel hostname → `/etc/hostname` → `$HOSTNAME`), mirroring the daemon's
/// resolver; falls back to a fixed label so a token always has an issuer.
fn local_hostname() -> String {
    let from_file = |path: &str| {
        std::fs::read_to_string(path)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    from_file("/proc/sys/kernel/hostname")
        .or_else(|| from_file("/etc/hostname"))
        .or_else(|| {
            std::env::var("HOSTNAME")
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "adele-web-ui.local".to_string())
}

/// A trimmed, non-empty copy of `value`, or `None` — so a blank config string
/// falls back to the default rather than becoming the literal `iss`/`aud`.
fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// The OS username for the default token `aud` (`"<user>.adelie-ai"`).
fn current_username() -> String {
    std::env::var("USER")
        .ok()
        .or_else(|| std::env::var("LOGNAME").ok())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "desktop-user".to_string())
}

impl BffConfig {
    /// Resolve the bind `SocketAddr` from `bind_address` + `port`.
    pub fn socket_addr(&self) -> anyhow::Result<SocketAddr> {
        Ok(format!("{}:{}", self.bind_address, self.port).parse()?)
    }

    /// Resolve the token `iss`: the configured value, else the local hostname.
    pub fn issuer(&self) -> String {
        non_empty(self.issuer.as_deref()).unwrap_or_else(local_hostname)
    }

    /// Resolve the token `aud`: the configured value, else `"<user>.adelie-ai"`.
    pub fn audience(&self) -> String {
        non_empty(self.audience.as_deref())
            .unwrap_or_else(|| format!("{}.adelie-ai", current_username()))
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
