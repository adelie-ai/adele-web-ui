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

/// `localStorage` key for the persisted session token.
const TOKEN_KEY: &str = "adele.token";

#[derive(Deserialize)]
struct LoginResponse {
    token: String,
}

/// The persisted token from a previous session, if any.
pub fn load_token() -> Option<String> {
    LocalStorage::get::<String>(TOKEN_KEY).ok()
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
