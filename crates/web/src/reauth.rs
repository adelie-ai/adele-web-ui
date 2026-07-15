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
//!
//! 2. [`on_attempt`] — the *reactive* fallback for a non-expired-but-rejected
//!    token. A rejected upgrade closes the socket before it ever opens; we fold
//!    each attempt's outcome into a consecutive "closed-before-open" streak and,
//!    once enough accrue with no working session between them, return to login.
//!
//! NOTE: this is the failing-spec commit — the bodies below are placeholder
//! stubs so the tests compile and run **red**; the real logic lands next.

// --- Layer 1: pre-emptive JWT expiry classification --------------------------

/// Treat a token as expired this many seconds *before* its actual `exp`.
pub const DEFAULT_SKEW_SECS: u64 = 30;

/// Classification of a stored token's `exp` claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenExpiry {
    /// Decoded, carries a numeric `exp`, and it is comfortably in the future.
    Valid,
    /// Decoded, carries a numeric `exp`, and it is at/past now (within skew).
    Expired,
    /// Couldn't determine expiry: not a decodable JWT, or no numeric `exp`.
    Unknown,
}

/// Classify `token` relative to `now_secs`, treating it as expired once it is
/// within `skew_secs` of (or past) its `exp`.
pub fn classify_token_expiry(token: &str, now_secs: u64, skew_secs: u64) -> TokenExpiry {
    let _ = (token, now_secs, skew_secs);
    TokenExpiry::Unknown
}

// --- Layer 2: reactive reconnect / auth-bail policy --------------------------

/// The outcome of one connection attempt, reduced to what the reconnect policy
/// needs (transport-type-free so it's host-testable).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptOutcome {
    /// The socket reached OPEN — the token was accepted and a real session began.
    Opened,
    /// The upgrade resolved to a close *before ever opening*.
    RejectedBeforeOpen,
    /// The socket never resolved / couldn't be constructed — a connectivity
    /// problem, not an auth one.
    NetworkError,
}

/// What the session loop should do after folding an attempt into the streak.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconnectAction {
    /// A working session ran: reset backoff and (re)connect.
    Reconnect,
    /// Keep retrying with backoff.
    Retry,
    /// The token is being refused: clear it and return to the login screen.
    ReturnToLogin,
}

/// Consecutive close-before-open failures tolerated before dropping to login.
pub const MAX_REJECTED_ATTEMPTS: u32 = 3;

/// Fold one attempt outcome into the running reject-before-open streak.
pub fn on_attempt(streak: u32, outcome: AttemptOutcome) -> (u32, ReconnectAction) {
    let _ = (streak, outcome);
    (0, ReconnectAction::Retry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;

    /// Build a JWT-shaped `header.payload.sig` string whose payload is exactly
    /// `payload_json` (base64url, no padding — matching `jsonwebtoken`'s output).
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
        assert_eq!(classify_token_expiry(&token, NOW, SKEW), TokenExpiry::Expired);
    }

    #[test]
    fn exp_within_skew_is_expired() {
        let token = jwt(&format!(r#"{{"sub":"u","exp":{}}}"#, NOW + 10));
        assert_eq!(classify_token_expiry(&token, NOW, SKEW), TokenExpiry::Expired);
    }

    #[test]
    fn exp_just_beyond_skew_is_valid() {
        let token = jwt(&format!(r#"{{"sub":"u","exp":{}}}"#, NOW + SKEW + 1));
        assert_eq!(classify_token_expiry(&token, NOW, SKEW), TokenExpiry::Valid);
    }

    #[test]
    fn exp_exactly_at_skew_boundary_is_expired() {
        let token = jwt(&format!(r#"{{"sub":"u","exp":{}}}"#, NOW + SKEW));
        assert_eq!(classify_token_expiry(&token, NOW, SKEW), TokenExpiry::Expired);
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
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"just text");
        let token = format!("aaa.{payload}.bbb");
        assert_eq!(
            classify_token_expiry(&token, NOW, SKEW),
            TokenExpiry::Unknown
        );
    }

    #[test]
    fn non_numeric_exp_is_unknown() {
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
            on_attempt(MAX_REJECTED_ATTEMPTS - 1, AttemptOutcome::RejectedBeforeOpen),
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
        assert_eq!(
            on_attempt(99, AttemptOutcome::NetworkError),
            (99, ReconnectAction::Retry)
        );
    }

    #[test]
    fn open_between_rejects_prevents_bail() {
        let (s, _) = on_attempt(0, AttemptOutcome::RejectedBeforeOpen);
        let (s, _) = on_attempt(s, AttemptOutcome::RejectedBeforeOpen);
        let (s, a) = on_attempt(s, AttemptOutcome::Opened);
        assert_eq!((s, a), (0, ReconnectAction::Reconnect));
        let (s, a) = on_attempt(s, AttemptOutcome::RejectedBeforeOpen);
        assert_eq!((s, a), (1, ReconnectAction::Retry));
    }
}
