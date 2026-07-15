//! Pure re-auth primitives behind graceful recovery from a rejected/expired
//! session token (issue #42), kept transport- and browser-free so they compile
//! and unit-test on the host target (like [`crate::wire`] and [`crate::reply`]).
//!
//! Two decisions the session loop needs, isolated here as pure functions:
//!
//! 1. [`classify_token_expiry`] — a *pre-emptive* check. The session token is a
//!    JWT; before connecting (on load and before each reconnect) we base64url-
//!    decode its payload — **no signature verification, we only read `exp`** —
//!    and, if it is already past (within a small clock-skew margin), drop
//!    straight to the login screen instead of attempting a doomed `/ws` upgrade.
//!    A token whose `exp` can't be read is [`TokenExpiry::Unknown`], NOT expired,
//!    so a decode quirk can never lock a user out — the reactive layer below is
//!    the backstop.
//!
//! 2. [`on_attempt`] — the *reactive* fallback for a non-expired-but-rejected
//!    token (e.g. the BFF's signing key rotated). A rejected upgrade closes the
//!    socket before it ever opens, indistinguishable in the browser from an
//!    unreachable BFF. We fold each attempt's outcome into a consecutive
//!    "closed-before-open" streak: reaching OPEN (the token was accepted) resets
//!    it, and once enough *consecutive* rejects accrue with **no working session
//!    between them** we conclude the token is bad and return to login. A plain
//!    network stall never counts, so healthy reconnects (phones sleeping /
//!    changing networks) keep looping forever as before.

use serde::Deserialize;

// --- Layer 1: pre-emptive JWT expiry classification --------------------------

/// Treat a token as expired this many seconds *before* its actual `exp`. Covers
/// client/server clock drift and the round-trip of the upgrade, so we never
/// present a token that would die mid-handshake. Deliberately small.
pub const DEFAULT_SKEW_SECS: u64 = 30;

/// Classification of a stored token's `exp` claim. Only [`Expired`](Self::Expired)
/// is actionable for the pre-emptive drop-to-login; [`Unknown`](Self::Unknown)
/// (unreadable or absent `exp`) falls through to a real connect on purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenExpiry {
    /// Decoded, carries a numeric `exp`, and it is comfortably in the future.
    Valid,
    /// Decoded, carries a numeric `exp`, and it is at/past now (within skew).
    Expired,
    /// Couldn't determine expiry: not a decodable JWT, or no numeric `exp`.
    Unknown,
}

/// The subset of JWT claims we read. Everything else (sub/iss/sig/…) is ignored;
/// we are not verifying the token, only inspecting its self-declared lifetime.
#[derive(Deserialize)]
struct ExpClaim {
    /// RFC 7519 `exp` (NumericDate, seconds since the Unix epoch). `f64` tolerates
    /// a fractional value; absent → `None` → [`TokenExpiry::Unknown`].
    exp: Option<f64>,
}

/// Classify `token` relative to `now_secs`, treating it as expired once it is
/// within `skew_secs` of (or past) its `exp`. Pure and verification-free.
pub fn classify_token_expiry(token: &str, now_secs: u64, skew_secs: u64) -> TokenExpiry {
    let Some(exp) = decode_exp(token) else {
        return TokenExpiry::Unknown;
    };
    // Expired when `exp <= now + skew`, i.e. `now >= exp - skew`. The server
    // rejects at `now_server > exp`; if our clock trails the server's by up to
    // `skew`, that happens once `now_local + skew > exp`, so pre-empting here
    // avoids a refusal we can already see coming.
    let deadline = now_secs.saturating_add(skew_secs) as f64;
    if exp <= deadline {
        TokenExpiry::Expired
    } else {
        TokenExpiry::Valid
    }
}

/// Base64url-decode a JWT payload (the 2nd of three dot-separated segments) and
/// read its `exp`. `None` for anything that isn't a well-formed JWT carrying a
/// finite, non-negative numeric `exp`.
fn decode_exp(token: &str) -> Option<f64> {
    use base64::Engine as _;

    let mut parts = token.split('.');
    let _header = parts.next()?;
    let payload = parts.next()?;
    let _signature = parts.next()?;
    // A real JWT has exactly three segments; more means it isn't one.
    if parts.next().is_some() {
        return None;
    }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let claim: ExpClaim = serde_json::from_slice(&bytes).ok()?;
    let exp = claim.exp?;
    // Reject NaN/inf/negative so a corrupt claim reads as Unknown, never as a
    // spuriously-valid far-future token.
    (exp.is_finite() && exp >= 0.0).then_some(exp)
}

// --- Layer 2: reactive reconnect / auth-bail policy --------------------------

/// The outcome of one connection attempt, reduced to what the reconnect policy
/// needs (transport-type-free so it's host-testable).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptOutcome {
    /// The socket reached OPEN — the token was accepted and a real session began
    /// (however briefly). This is the "working state" that clears the streak.
    Opened,
    /// The upgrade resolved to a close *before ever opening* — the BFF refused
    /// the token (expiry/rotation/revocation) or is unreachable (the browser
    /// can't tell these apart). Counts toward the auth-bail streak.
    RejectedBeforeOpen,
    /// The socket never resolved (still connecting past the cap) or couldn't be
    /// constructed at all — a connectivity problem, not an auth one. Retried with
    /// backoff; never contributes to the auth-bail streak.
    NetworkError,
}

/// What the session loop should do after folding an attempt into the streak.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconnectAction {
    /// A working session ran: reset backoff and (re)connect.
    Reconnect,
    /// Keep retrying with backoff (a network hiccup, or an under-threshold
    /// close-before-open that isn't yet conclusive).
    Retry,
    /// Enough consecutive rejects with no working session between them: the token
    /// is being refused. Clear it and return to the login screen.
    ReturnToLogin,
}

/// Consecutive close-before-open failures (with no [`Opened`](AttemptOutcome::Opened)
/// between them) tolerated before we conclude the token is bad and drop to login.
/// Small, so recovery is fast, but > 1 so a single stray close doesn't log anyone
/// out.
pub const MAX_REJECTED_ATTEMPTS: u32 = 3;

/// Fold one attempt outcome into the running reject-before-open streak, returning
/// the updated streak and the action to take. Pure, so the whole branch table —
/// healthy-reset, under-threshold retry, bail-at-threshold, network-never-bails —
/// is unit-tested on the host without a browser.
pub fn on_attempt(streak: u32, outcome: AttemptOutcome) -> (u32, ReconnectAction) {
    match outcome {
        AttemptOutcome::Opened => (0, ReconnectAction::Reconnect),
        AttemptOutcome::NetworkError => (streak, ReconnectAction::Retry),
        AttemptOutcome::RejectedBeforeOpen => {
            let streak = streak.saturating_add(1);
            if streak >= MAX_REJECTED_ATTEMPTS {
                (streak, ReconnectAction::ReturnToLogin)
            } else {
                (streak, ReconnectAction::Retry)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;

    /// Build a JWT-shaped `header.payload.sig` string whose payload is exactly
    /// `payload_json` (base64url, no padding — matching `jsonwebtoken`'s output).
    /// Header/signature are opaque placeholders: nothing here verifies them.
    fn jwt(payload_json: &str) -> String {
        let b64 = |s: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s);
        format!(
            "{}.{}.{}",
            b64(br#"{"alg":"HS256","typ":"JWT"}"#),
            b64(payload_json.as_bytes()),
            "c2lnbmF0dXJl"
        )
    }

    const NOW: u64 = 1_752_000_000; // a fixed "now" so the tests are deterministic
    const SKEW: u64 = DEFAULT_SKEW_SECS;

    // --- classify_token_expiry -------------------------------------------------

    #[test]
    fn valid_future_exp_is_valid() {
        let token = jwt(&format!(r#"{{"sub":"u","exp":{}}}"#, NOW + 3600));
        assert_eq!(classify_token_expiry(&token, NOW, SKEW), TokenExpiry::Valid);
    }

    #[test]
    fn past_exp_is_expired() {
        let token = jwt(&format!(r#"{{"sub":"u","exp":{}}}"#, NOW - 10));
        assert_eq!(
            classify_token_expiry(&token, NOW, SKEW),
            TokenExpiry::Expired
        );
    }

    #[test]
    fn exp_within_skew_is_expired() {
        // Ten seconds in the future, but inside the 30s skew window: the server
        // would refuse it before we finished connecting, so pre-empt it.
        let token = jwt(&format!(r#"{{"sub":"u","exp":{}}}"#, NOW + 10));
        assert_eq!(
            classify_token_expiry(&token, NOW, SKEW),
            TokenExpiry::Expired
        );
    }

    #[test]
    fn exp_just_beyond_skew_is_valid() {
        // One second past the skew boundary must stay Valid (boundary is inclusive
        // on the expired side: exp <= now + skew).
        let token = jwt(&format!(r#"{{"sub":"u","exp":{}}}"#, NOW + SKEW + 1));
        assert_eq!(classify_token_expiry(&token, NOW, SKEW), TokenExpiry::Valid);
    }

    #[test]
    fn exp_exactly_at_skew_boundary_is_expired() {
        let token = jwt(&format!(r#"{{"sub":"u","exp":{}}}"#, NOW + SKEW));
        assert_eq!(
            classify_token_expiry(&token, NOW, SKEW),
            TokenExpiry::Expired
        );
    }

    #[test]
    fn malformed_token_is_unknown() {
        assert_eq!(
            classify_token_expiry("not-a-jwt", NOW, SKEW),
            TokenExpiry::Unknown
        );
    }

    #[test]
    fn missing_exp_is_unknown() {
        // A valid JWT payload that simply has no `exp` claim.
        let token = jwt(r#"{"sub":"u","iat":1}"#);
        assert_eq!(
            classify_token_expiry(&token, NOW, SKEW),
            TokenExpiry::Unknown
        );
    }

    #[test]
    fn too_few_segments_is_unknown() {
        let token = jwt(r#"{"exp":1}"#);
        let two_segments = token.rsplit_once('.').unwrap().0.to_string();
        assert_eq!(
            classify_token_expiry(&two_segments, NOW, SKEW),
            TokenExpiry::Unknown
        );
    }

    #[test]
    fn non_base64_payload_is_unknown() {
        assert_eq!(
            classify_token_expiry("aaa.!!!not base64!!!.bbb", NOW, SKEW),
            TokenExpiry::Unknown
        );
    }

    #[test]
    fn payload_not_json_is_unknown() {
        use base64::Engine as _;
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"just text");
        let token = format!("aaa.{payload}.bbb");
        assert_eq!(
            classify_token_expiry(&token, NOW, SKEW),
            TokenExpiry::Unknown
        );
    }

    #[test]
    fn non_numeric_exp_is_unknown() {
        // `exp` present but a string — not a NumericDate; read as Unknown, never
        // as valid.
        let token = jwt(r#"{"exp":"soon"}"#);
        assert_eq!(
            classify_token_expiry(&token, NOW, SKEW),
            TokenExpiry::Unknown
        );
    }

    // --- on_attempt (reconnect / auth-bail policy) ----------------------------

    #[test]
    fn opened_resets_streak_and_reconnects() {
        assert_eq!(
            on_attempt(2, AttemptOutcome::Opened),
            (0, ReconnectAction::Reconnect)
        );
    }

    #[test]
    fn first_reject_retries() {
        assert_eq!(
            on_attempt(0, AttemptOutcome::RejectedBeforeOpen),
            (1, ReconnectAction::Retry)
        );
    }

    #[test]
    fn second_reject_below_threshold_retries() {
        assert_eq!(
            on_attempt(1, AttemptOutcome::RejectedBeforeOpen),
            (2, ReconnectAction::Retry)
        );
    }

    #[test]
    fn reject_reaching_threshold_returns_to_login() {
        assert_eq!(
            on_attempt(
                MAX_REJECTED_ATTEMPTS - 1,
                AttemptOutcome::RejectedBeforeOpen
            ),
            (MAX_REJECTED_ATTEMPTS, ReconnectAction::ReturnToLogin)
        );
    }

    #[test]
    fn network_error_retries_without_incrementing() {
        assert_eq!(
            on_attempt(2, AttemptOutcome::NetworkError),
            (2, ReconnectAction::Retry)
        );
    }

    #[test]
    fn network_error_never_bails_however_many() {
        // A BFF that's merely unreachable must never log the user out, no matter
        // how long it stays down.
        assert_eq!(
            on_attempt(99, AttemptOutcome::NetworkError),
            (99, ReconnectAction::Retry)
        );
    }

    #[test]
    fn open_between_rejects_prevents_bail() {
        // Reject, reject, then a working session, then reject again: the streak is
        // cleared by the open, so we're nowhere near the bail threshold — this is
        // the "healthy reconnect" that must survive intact.
        let (s, _) = on_attempt(0, AttemptOutcome::RejectedBeforeOpen);
        let (s, _) = on_attempt(s, AttemptOutcome::RejectedBeforeOpen);
        let (s, a) = on_attempt(s, AttemptOutcome::Opened);
        assert_eq!((s, a), (0, ReconnectAction::Reconnect));
        let (s, a) = on_attempt(s, AttemptOutcome::RejectedBeforeOpen);
        assert_eq!((s, a), (1, ReconnectAction::Retry));
    }
}
