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
/// Default directory of built SPA static assets, relative to the CWD (dev). In
/// the container this is overridden to `/srv/web` via `ADELE_WEB_UI_WEB_DIR`.
pub const DEFAULT_WEB_DIR: &str = "crates/web/dist";

/// Which transport the BFF uses to reach the daemon (the "back door").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DaemonTransport {
    /// Local Unix domain socket — a co-located daemon, authenticated by kernel
    /// peer-cred (desktop-assistant#407). Default; unchanged local behavior.
    #[default]
    Uds,
    /// WebSocket to a remote daemon (e.g. on k8s). Auth is the daemon's `/login`
    /// password exchange (or a pre-minted `daemon_ws_jwt`).
    Ws,
}

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
    /// Directory of built SPA static assets served at `/`. `None` ⇒
    /// [`DEFAULT_WEB_DIR`]. Absent-on-disk is tolerated (API-only; logged).
    pub web_dir: Option<PathBuf>,
    /// Transport used to reach the daemon (back door): UDS (default, co-located)
    /// or WS (a remote daemon, e.g. on k8s).
    pub daemon_transport: DaemonTransport,
    /// Daemon WebSocket URL for the WS back door, e.g.
    /// `ws://adele-daemon:11339/ws`. Required when `daemon_transport = ws`.
    pub daemon_ws_url: Option<String>,
    /// Username for the daemon's `/login` password exchange (WS back door).
    pub daemon_ws_username: Option<String>,
    /// Password for the daemon's `/login` password exchange (WS back door).
    pub daemon_ws_password: Option<String>,
    /// Pre-minted daemon JWT (WS back door). When set, used instead of `/login`.
    pub daemon_ws_jwt: Option<String>,
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
    /// HS256 signing key for the browser session tokens the BFF mints
    /// (`ADELE_WEB_UI_SIGNING_KEY`). When set (e.g. mounted from a k8s Secret)
    /// it is used directly, so the key is **stable across restarts/redeploys** —
    /// otherwise a random key is generated on disk per process, and every k8s
    /// deploy would then invalidate every outstanding browser token.
    pub signing_key: Option<String>,
    /// Browser session-token lifetime in seconds (`ADELE_WEB_UI_TOKEN_TTL_SECS`).
    /// `None`/absent ⇒ `auth::DEFAULT_TOKEN_TTL_SECS` (7 days).
    pub token_ttl_secs: Option<u64>,
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
            web_dir: None,
            daemon_transport: DaemonTransport::Uds,
            daemon_ws_url: None,
            daemon_ws_username: None,
            daemon_ws_password: None,
            daemon_ws_jwt: None,
            uds_socket: None,
            issuer: None,
            audience: None,
            signing_key: None,
            token_ttl_secs: None,
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

    /// Resolve the SPA static-asset dir: the configured value, else
    /// [`DEFAULT_WEB_DIR`].
    pub fn web_dir(&self) -> PathBuf {
        self.web_dir
            .clone()
            .unwrap_or_else(|| PathBuf::from(DEFAULT_WEB_DIR))
    }

    /// Overlay `ADELE_WEB_UI_*` environment variables on top of the loaded TOML
    /// (env wins), so a container or systemd unit can configure the service with
    /// no config file. Unset/blank vars leave the current value untouched.
    pub fn apply_env_overrides(&mut self) {
        self.apply_overrides_from(|key| std::env::var(key).ok());
    }

    /// Pure core of [`apply_env_overrides`], parameterized over the lookup so it
    /// is unit-testable without mutating the process environment.
    fn apply_overrides_from(&mut self, get: impl Fn(&str) -> Option<String>) {
        let s = |key: &str| {
            get(key)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        if let Some(v) = s("ADELE_WEB_UI_ENABLED") {
            self.enabled = matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on");
        }
        if let Some(v) = s("ADELE_WEB_UI_BIND_ADDRESS") {
            self.bind_address = v;
        }
        if let Some(v) = s("ADELE_WEB_UI_PORT")
            && let Ok(port) = v.parse()
        {
            self.port = port;
        }
        if let Some(v) = s("ADELE_WEB_UI_ALLOWED_ORIGINS") {
            self.allowed_origins = v
                .split(',')
                .map(|o| o.trim().to_string())
                .filter(|o| !o.is_empty())
                .collect();
        }
        if let Some(v) = s("ADELE_WEB_UI_LOGIN_USERNAME") {
            self.login_username = v;
        }
        if let Some(v) = s("ADELE_WEB_UI_LOGIN_PASSWORD") {
            self.login_password = Some(v);
        }
        if let Some(v) = s("ADELE_WEB_UI_ISSUER") {
            self.issuer = Some(v);
        }
        if let Some(v) = s("ADELE_WEB_UI_AUDIENCE") {
            self.audience = Some(v);
        }
        if let Some(v) = s("ADELE_WEB_UI_WEB_DIR") {
            self.web_dir = Some(PathBuf::from(v));
        }
        if let Some(v) = s("ADELE_WEB_UI_UDS_SOCKET") {
            self.uds_socket = Some(PathBuf::from(v));
        }
        // --- Daemon back-door transport --------------------------------------
        if let Some(v) = s("ADELE_WEB_UI_DAEMON_TRANSPORT") {
            match v.to_ascii_lowercase().as_str() {
                "ws" => self.daemon_transport = DaemonTransport::Ws,
                "uds" => self.daemon_transport = DaemonTransport::Uds,
                other => {
                    tracing::warn!(
                        transport = other,
                        "unknown ADELE_WEB_UI_DAEMON_TRANSPORT (want ws|uds); keeping current"
                    );
                }
            }
        }
        if let Some(v) = s("ADELE_WEB_UI_DAEMON_WS_URL") {
            self.daemon_ws_url = Some(v);
        }
        if let Some(v) = s("ADELE_WEB_UI_DAEMON_WS_USERNAME") {
            self.daemon_ws_username = Some(v);
        }
        if let Some(v) = s("ADELE_WEB_UI_DAEMON_WS_PASSWORD") {
            self.daemon_ws_password = Some(v);
        }
        if let Some(v) = s("ADELE_WEB_UI_DAEMON_WS_JWT") {
            self.daemon_ws_jwt = Some(v);
        }
        if let Some(v) = s("ADELE_WEB_UI_SIGNING_KEY") {
            self.signing_key = Some(v);
        }
        if let Some(v) = s("ADELE_WEB_UI_TOKEN_TTL_SECS") {
            match v.parse::<u64>() {
                Ok(secs) => self.token_ttl_secs = Some(secs),
                Err(_) => tracing::warn!(
                    "invalid ADELE_WEB_UI_TOKEN_TTL_SECS (want a positive integer); keeping current"
                ),
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Build a `get` closure over a fixed map for the pure override core.
    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |key: &str| map.get(key).cloned()
    }

    #[test]
    fn defaults_are_uds_and_disabled() {
        let cfg = BffConfig::default();
        assert!(!cfg.enabled);
        assert_eq!(cfg.daemon_transport, DaemonTransport::Uds);
        assert_eq!(cfg.web_dir(), PathBuf::from(DEFAULT_WEB_DIR));
    }

    #[test]
    fn env_overrides_configure_ws_back_door() {
        let mut cfg = BffConfig::default();
        cfg.apply_overrides_from(env(&[
            ("ADELE_WEB_UI_ENABLED", "true"),
            ("ADELE_WEB_UI_BIND_ADDRESS", "0.0.0.0"),
            ("ADELE_WEB_UI_PORT", "9379"),
            (
                "ADELE_WEB_UI_ALLOWED_ORIGINS",
                "http://localhost:9379, http://127.0.0.1:9379",
            ),
            ("ADELE_WEB_UI_DAEMON_TRANSPORT", "ws"),
            ("ADELE_WEB_UI_DAEMON_WS_URL", "ws://adele-daemon:11339/ws"),
            ("ADELE_WEB_UI_DAEMON_WS_USERNAME", "adele"),
            ("ADELE_WEB_UI_DAEMON_WS_PASSWORD", "s3cret"),
            ("ADELE_WEB_UI_WEB_DIR", "/srv/web"),
        ]));

        assert!(cfg.enabled);
        assert_eq!(cfg.bind_address, "0.0.0.0");
        assert_eq!(cfg.port, 9379);
        assert_eq!(
            cfg.allowed_origins,
            vec![
                "http://localhost:9379".to_string(),
                "http://127.0.0.1:9379".to_string(),
            ]
        );
        assert_eq!(cfg.daemon_transport, DaemonTransport::Ws);
        assert_eq!(
            cfg.daemon_ws_url.as_deref(),
            Some("ws://adele-daemon:11339/ws")
        );
        assert_eq!(cfg.daemon_ws_username.as_deref(), Some("adele"));
        assert_eq!(cfg.daemon_ws_password.as_deref(), Some("s3cret"));
        assert_eq!(cfg.web_dir(), PathBuf::from("/srv/web"));
    }

    #[test]
    fn env_overrides_signing_key_and_token_ttl() {
        let key = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let mut cfg = BffConfig::default();
        cfg.apply_overrides_from(env(&[
            ("ADELE_WEB_UI_SIGNING_KEY", key),
            ("ADELE_WEB_UI_TOKEN_TTL_SECS", "604800"),
        ]));
        // A configured key makes the signing key stable across restarts/redeploys.
        assert_eq!(cfg.signing_key.as_deref(), Some(key));
        assert_eq!(cfg.token_ttl_secs, Some(604_800));
    }

    #[test]
    fn invalid_token_ttl_is_ignored() {
        let mut cfg = BffConfig::default();
        cfg.apply_overrides_from(env(&[("ADELE_WEB_UI_TOKEN_TTL_SECS", "not-a-number")]));
        assert_eq!(cfg.token_ttl_secs, None);
    }

    #[test]
    fn empty_env_leaves_values_untouched() {
        let mut cfg = BffConfig {
            login_username: "preset".to_string(),
            ..Default::default()
        };
        // Present-but-blank must not clobber; absent must not clobber.
        cfg.apply_overrides_from(env(&[("ADELE_WEB_UI_LOGIN_USERNAME", "  ")]));
        assert_eq!(cfg.login_username, "preset");
        assert_eq!(cfg.daemon_transport, DaemonTransport::Uds);
    }

    #[test]
    fn unknown_transport_keeps_current() {
        let mut cfg = BffConfig::default();
        cfg.apply_overrides_from(env(&[("ADELE_WEB_UI_DAEMON_TRANSPORT", "carrier-pigeon")]));
        assert_eq!(cfg.daemon_transport, DaemonTransport::Uds);
    }

    #[test]
    fn enabled_accepts_common_truthy_values_only() {
        for truthy in ["1", "true", "YES", "On"] {
            let mut cfg = BffConfig::default();
            cfg.apply_overrides_from(env(&[("ADELE_WEB_UI_ENABLED", truthy)]));
            assert!(cfg.enabled, "{truthy} should enable");
        }
        let mut cfg = BffConfig {
            enabled: true,
            ..Default::default()
        };
        cfg.apply_overrides_from(env(&[("ADELE_WEB_UI_ENABLED", "false")]));
        assert!(!cfg.enabled);
    }
}
