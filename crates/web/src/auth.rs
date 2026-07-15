//! Browser-facing auth: password login against the BFF's `POST /login` and JWT
//! persistence in `localStorage`.
//!
//! `/login` takes HTTP Basic credentials and returns `{ token, .. }`; the token
//! is then presented to `/ws` via the auth subprotocol (see `transport`). The
//! SPA is same-origin with the BFF, so the relative `/login` path resolves
//! correctly both in production and behind the Trunk dev proxy.

use gloo_net::http::Request;
use gloo_storage::{LocalStorage, Storage};
use serde::Deserialize;

use crate::reauth::{DEFAULT_SKEW_SECS, TokenExpiry, classify_token_expiry};

/// `localStorage` key for the persisted session token.
const TOKEN_KEY: &str = "adele.token";

#[derive(Deserialize)]
struct LoginResponse {
    token: String,
}

/// Current wall-clock time in whole seconds since the Unix epoch (browser clock),
/// for comparing against a token's `exp`.
fn now_secs() -> u64 {
    // `Date::now()` is milliseconds since the epoch; it is always finite and
    // positive in a browser, so the truncating cast is safe.
    (js_sys::Date::now() / 1000.0) as u64
}

/// Whether `token`'s `exp` is already past (within a small clock-skew margin).
/// A token whose expiry can't be read is **not** treated as expired here — the
/// connect path's fast-failure guard is the backstop, so a decode quirk never
/// locks anyone out pre-emptively.
pub fn token_is_expired(token: &str) -> bool {
    matches!(
        classify_token_expiry(token, now_secs(), DEFAULT_SKEW_SECS),
        TokenExpiry::Expired
    )
}

/// The persisted token from a previous session — but only if it isn't already
/// expired. An expired token is forgotten (cleared from storage) and `None`
/// returned, so the app opens on the login screen instead of attempting a doomed
/// `/ws` upgrade with a dead token (issue #42).
pub fn load_token() -> Option<String> {
    let token = LocalStorage::get::<String>(TOKEN_KEY).ok()?;
    if token_is_expired(&token) {
        clear_token();
        return None;
    }
    Some(token)
}

fn store_token(token: &str) {
    let _ = LocalStorage::set(TOKEN_KEY, token);
}

/// Forget the persisted token (sign-out, or after a 401).
pub fn clear_token() {
    LocalStorage::delete(TOKEN_KEY);
}

/// Exchange username/password for a JWT at `POST /login`, persisting it on
/// success. `Err` carries a human-readable reason for the login screen.
pub async fn login(username: &str, password: &str) -> Result<String, String> {
    let resp = Request::post("/login")
        .header("authorization", &basic_auth(username, password)?)
        .send()
        .await
        .map_err(|e| format!("login request failed: {e}"))?;

    if resp.status() == 401 {
        return Err("Incorrect username or password.".to_string());
    }
    if !resp.ok() {
        return Err(format!("Login failed ({}).", resp.status()));
    }

    let body: LoginResponse = resp
        .json()
        .await
        .map_err(|e| format!("unexpected login response: {e}"))?;
    store_token(&body.token);
    Ok(body.token)
}

/// `Basic base64(user:pass)` using the browser's `btoa` (credentials are ASCII).
fn basic_auth(username: &str, password: &str) -> Result<String, String> {
    let window = web_sys::window().ok_or("no window object")?;
    let encoded = window
        .btoa(&format!("{username}:{password}"))
        .map_err(|_| "failed to encode credentials".to_string())?;
    Ok(format!("Basic {encoded}"))
}
