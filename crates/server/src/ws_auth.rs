//! Browser WebSocket-auth bridge.
//!
//! The daemon's `ws-interface` authenticates `/ws` from the `Authorization:
//! Bearer` header (see its `ws_handler`). That's right for its native UDS/CLI
//! clients, but a **browser cannot set request headers on a WebSocket
//! handshake** — the `WebSocket` API only lets it offer *subprotocols*. So the
//! SPA connects with `new WebSocket(url, [BEARER_SUBPROTOCOL, <jwt>])`, and this
//! BFF-only middleware relays that token into the `Authorization` header the
//! embedded router expects, then echoes the sentinel subprotocol back on the
//! `101` so the browser accepts the upgrade.
//!
//! This is a *relay, not a new trust path*: it only adds `Authorization` when
//! one is absent, and the JWT is still validated by `ws-interface`
//! (`validate_bearer_token`) and gated by the same `Origin` allowlist. Keeping
//! it in the BFF leaves the daemon's ws-interface contract (Bearer-only)
//! untouched — browser quirks belong to the backend-for-frontend.

use axum::extract::Request;
use axum::http::header::{AUTHORIZATION, SEC_WEBSOCKET_PROTOCOL};
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::Response;

/// Sentinel subprotocol the SPA offers alongside the raw JWT, so the server can
/// tell which offered protocol is the marker (echoed back) and which is the
/// token (consumed). Mirrored by the web client.
pub const BEARER_SUBPROTOCOL: &str = "adele.bearer";

/// Path of the embedded ws-interface WebSocket route.
const WS_PATH: &str = "/ws";

/// Middleware: for a browser `/ws` upgrade that carries its bearer token in
/// `Sec-WebSocket-Protocol`, inject `Authorization: Bearer <token>` so the
/// embedded ws router authenticates it, and echo [`BEARER_SUBPROTOCOL`] on the
/// `101 Switching Protocols` response (a browser fails the socket if the server
/// selects none of the subprotocols it offered).
///
/// A no-op for every other request: non-`/ws` paths, requests that already carry
/// `Authorization` (native clients), and upgrades without the sentinel.
pub async fn inject_bearer_from_subprotocol(mut req: Request, next: Next) -> Response {
    let token = bearer_token_to_inject(
        req.uri().path(),
        req.headers().contains_key(AUTHORIZATION),
        req.headers()
            .get(SEC_WEBSOCKET_PROTOCOL)
            .and_then(|v| v.to_str().ok()),
    );

    // Only echo the sentinel if we actually injected a (header-valid) token.
    let injected = token
        .and_then(|tok| HeaderValue::from_str(&format!("Bearer {tok}")).ok())
        .map(|auth| req.headers_mut().insert(AUTHORIZATION, auth))
        .is_some();

    let mut res = next.run(req).await;

    if injected && res.status() == StatusCode::SWITCHING_PROTOCOLS {
        res.headers_mut().insert(
            SEC_WEBSOCKET_PROTOCOL,
            HeaderValue::from_static(BEARER_SUBPROTOCOL),
        );
    }
    res
}

/// Decide the bearer token (if any) to relay into `Authorization` for a request.
/// Pure so it can be unit-tested without the axum middleware machinery.
///
/// Returns `None` — leave the request untouched — unless this is a `/ws` request
/// with no existing `Authorization` whose `Sec-WebSocket-Protocol` carries the
/// sentinel plus a token.
fn bearer_token_to_inject(
    path: &str,
    has_authorization: bool,
    subprotocols: Option<&str>,
) -> Option<String> {
    if path != WS_PATH || has_authorization {
        return None;
    }
    token_from_subprotocols(subprotocols?)
}

/// Extract the JWT from a `Sec-WebSocket-Protocol` header value of the form
/// `"adele.bearer, <jwt>"` (order-independent): the sentinel must be present,
/// and the token is the first other protocol offered.
fn token_from_subprotocols(header: &str) -> Option<String> {
    let protocols: Vec<&str> = header
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if !protocols.contains(&BEARER_SUBPROTOCOL) {
        return None;
    }
    protocols
        .into_iter()
        .find(|p| *p != BEARER_SUBPROTOCOL)
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_token_when_sentinel_present() {
        assert_eq!(
            token_from_subprotocols("adele.bearer, jwt-abc"),
            Some("jwt-abc".to_string())
        );
    }

    #[test]
    fn token_extraction_is_order_independent() {
        assert_eq!(
            token_from_subprotocols("jwt-xyz, adele.bearer"),
            Some("jwt-xyz".to_string())
        );
    }

    #[test]
    fn no_sentinel_yields_no_token() {
        // A lone protocol that isn't the sentinel is not treated as a token —
        // avoids mistaking some other subprotocol for credentials.
        assert_eq!(token_from_subprotocols("graphql-ws"), None);
    }

    #[test]
    fn sentinel_without_token_yields_none() {
        assert_eq!(token_from_subprotocols("adele.bearer"), None);
    }

    #[test]
    fn empty_header_yields_none() {
        assert_eq!(token_from_subprotocols(""), None);
        assert_eq!(token_from_subprotocols("   ,  "), None);
    }

    #[test]
    fn injects_only_for_ws_path() {
        assert_eq!(
            bearer_token_to_inject("/ws", false, Some("adele.bearer, jwt")),
            Some("jwt".to_string())
        );
        assert_eq!(
            bearer_token_to_inject("/login", false, Some("adele.bearer, jwt")),
            None
        );
        assert_eq!(
            bearer_token_to_inject("/healthz", false, Some("adele.bearer, jwt")),
            None
        );
    }

    #[test]
    fn never_overrides_existing_authorization() {
        // A native client that already sent a Bearer header is left alone.
        assert_eq!(
            bearer_token_to_inject("/ws", true, Some("adele.bearer, jwt")),
            None
        );
    }

    #[test]
    fn ws_without_subprotocol_is_untouched() {
        assert_eq!(bearer_token_to_inject("/ws", false, None), None);
    }
}
