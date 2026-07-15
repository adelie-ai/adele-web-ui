//! Browser-facing auth for the BFF: a JWT bearer validator and a password
//! login service, both built on `desktop-assistant-auth-jwt` (HS256, shared
//! signing key). The BFF mints its own browser session tokens and validates
//! them; over UDS the daemon authenticates the BFF separately (peer-UID).
//!
//! Login is config-driven: a static password today (PAM/system auth is a
//! follow-up — it lives privately in the daemon and needs extracting to a
//! shared crate).

use std::time::{SystemTime, UNIX_EPOCH};

use desktop_assistant_auth_jwt::{Claims, UserId, decode, encode};
use desktop_assistant_ws::{WsAuthValidator, WsLoginService};

/// Default browser session-token lifetime — 7 days. A 15-minute TTL combined
/// with a signing key that regenerates on every k8s deploy stranded the SPA on
/// an invalid token (it kept retry-spamming the `/ws` upgrade). 7 days is a sane
/// lifetime for a single-user tailnet service; override via
/// `ADELE_WEB_UI_TOKEN_TTL_SECS`.
pub const DEFAULT_TOKEN_TTL_SECS: u64 = 7 * 24 * 60 * 60;

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Validates browser bearer tokens (HS256, shared signing key). The `issuer` /
/// `audience` are config-resolved and shared with [`PasswordLogin`] so issue and
/// validate can't drift.
pub struct JwtValidator {
    signing_key: String,
    issuer: String,
    audience: String,
}

impl JwtValidator {
    pub fn new(signing_key: String, issuer: String, audience: String) -> Self {
        Self {
            signing_key,
            issuer,
            audience,
        }
    }
}

#[async_trait::async_trait]
impl WsAuthValidator for JwtValidator {
    async fn validate_bearer_token(&self, token: &str) -> bool {
        decode(token, &self.signing_key, &self.issuer, &self.audience).is_ok()
    }

    async fn extract_user_id(&self, token: &str) -> Option<UserId> {
        decode(token, &self.signing_key, &self.issuer, &self.audience)
            .ok()
            .map(|claims| UserId::new(claims.sub))
    }
}

/// Static-password login backing `POST /login`. Issues HS256 session tokens
/// stamped with the config-resolved `issuer` / `audience`.
pub struct PasswordLogin {
    username: String,
    password: String,
    signing_key: String,
    issuer: String,
    audience: String,
    token_ttl_secs: u64,
}

impl PasswordLogin {
    pub fn new(
        username: String,
        password: String,
        signing_key: String,
        issuer: String,
        audience: String,
        token_ttl_secs: u64,
    ) -> Self {
        Self {
            username,
            password,
            signing_key,
            issuer,
            audience,
            token_ttl_secs,
        }
    }
}

#[async_trait::async_trait]
impl WsLoginService for PasswordLogin {
    async fn authenticate_basic(&self, username: &str, password: &str) -> bool {
        // Length-leaking but constant enough for a single-user LAN service;
        // tighten (constant-time compare) if multi-user/PAM lands.
        username == self.username && password == self.password
    }

    async fn issue_token_for_subject(&self, subject: &str) -> Result<String, String> {
        let iat = now_secs();
        let claims = Claims {
            iss: self.issuer.clone(),
            sub: subject.to_string(),
            aud: self.audience.clone(),
            exp: iat + self.token_ttl_secs,
            iat,
            nbf: iat,
            jti: uuid::Uuid::new_v4().simple().to_string(),
        };
        encode(&claims, &self.signing_key).map_err(|e| e.to_string())
    }
}
