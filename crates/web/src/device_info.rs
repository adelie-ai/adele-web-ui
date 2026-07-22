//! Browser-scoped client context (#557).
//!
//! A browser knows only two things worth grounding the assistant with: the
//! user's **timezone** (so "tonight" / "this morning" resolve in their zone) and
//! a **coarse OS** family. It cannot know a home directory, username, or
//! hostname — and the BFF's own server environment is the wrong machine — so
//! those are NEVER resolved or sent (adele-web-ui#64 already forces the BFF's
//! daemon connection to share nothing). This module resolves the two knowable
//! fields in the wasm client and rides them on each turn's `SendMessage`, gated
//! by a **default-on** "Share device info" toggle (an opt-out).
//!
//! Pattern mirrors [`crate::read_aloud`] / [`crate::context`]: the pure mapping
//! core is host-testable; the `#[cfg(target_arch = "wasm32")]` submodule owns
//! the browser glue (Intl timezone, `navigator` OS, localStorage persistence,
//! and the settings toggle).

use desktop_assistant_api_model::ClientContext;

/// Default for the "Share device info" toggle: sharing is **on** by default —
/// it is an opt-*out*, so a browser user gets correct local-time grounding
/// without having to enable anything (#557).
#[cfg_attr(not(test), allow(dead_code))]
pub const SHARE_DEVICE_INFO_DEFAULT: bool = true;

/// Build the browser-scoped [`ClientContext`] from a resolved timezone and OS.
///
/// Only `timezone` + `os` are ever populated — a browser cannot know the
/// account/device fields, so they stay absent. Blank values are dropped, and an
/// all-absent result is `None` so the SPA sends `client_context: None` rather
/// than an empty object.
#[cfg_attr(not(test), allow(dead_code))]
pub fn browser_client_context(
    timezone: Option<String>,
    os: Option<String>,
) -> Option<ClientContext> {
    // STUB (spec): the real mapping lands in the implementation commit.
    let _ = (timezone, os);
    None
}

/// Map a browser's `navigator.platform` + `navigator.userAgent` to a coarse OS
/// family (`"macOS"`, `"Windows"`, `"Linux"`, `"Android"`, `"iOS"`), falling
/// back to the raw platform string when unrecognised and `None` when both inputs
/// are blank.
#[cfg_attr(not(test), allow(dead_code))]
pub fn coarse_os(platform: &str, user_agent: &str) -> Option<String> {
    // STUB (spec): the real mapping lands in the implementation commit.
    let _ = (platform, user_agent);
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_default_is_on() {
        // Sharing device info is opt-OUT: on by default (#557).
        assert!(SHARE_DEVICE_INFO_DEFAULT);
    }

    #[test]
    fn builds_context_with_only_timezone_and_os() {
        let ctx = browser_client_context(Some("Europe/London".into()), Some("macOS".into()))
            .expect("some context");
        assert_eq!(ctx.timezone.as_deref(), Some("Europe/London"));
        assert_eq!(ctx.os.as_deref(), Some("macOS"));
        // A browser can never know these; they must stay absent.
        assert_eq!(ctx.real_name, None);
        assert_eq!(ctx.username, None);
        assert_eq!(ctx.home_dir, None);
        assert_eq!(ctx.hostname, None);
    }

    #[test]
    fn absent_timezone_and_os_is_none() {
        assert_eq!(browser_client_context(None, None), None);
    }

    #[test]
    fn blank_values_are_dropped_to_none() {
        assert_eq!(
            browser_client_context(Some("   ".into()), Some(String::new())),
            None
        );
    }

    #[test]
    fn timezone_only_is_kept() {
        let ctx =
            browser_client_context(Some("America/New_York".into()), None).expect("some context");
        assert_eq!(ctx.timezone.as_deref(), Some("America/New_York"));
        assert_eq!(ctx.os, None);
    }

    #[test]
    fn coarse_os_recognises_common_families() {
        assert_eq!(coarse_os("MacIntel", "").as_deref(), Some("macOS"));
        assert_eq!(coarse_os("Win32", "").as_deref(), Some("Windows"));
        assert_eq!(
            coarse_os("Linux x86_64", "Mozilla/5.0 (X11; Linux x86_64)").as_deref(),
            Some("Linux")
        );
        assert_eq!(coarse_os("iPhone", "").as_deref(), Some("iOS"));
        // Android reports a Linux platform; the user-agent disambiguates it.
        assert_eq!(
            coarse_os("Linux armv8l", "Mozilla/5.0 (Linux; Android 14)").as_deref(),
            Some("Android")
        );
    }

    #[test]
    fn coarse_os_passes_through_unknown_platform() {
        assert_eq!(coarse_os("SomeFutureOS", "").as_deref(), Some("SomeFutureOS"));
    }

    #[test]
    fn coarse_os_empty_is_none() {
        assert_eq!(coarse_os("", ""), None);
    }
}
