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
pub const SHARE_DEVICE_INFO_DEFAULT: bool = true;

/// Trim a resolved value to `Some(non-blank)` or `None`.
fn clean(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Build the browser-scoped [`ClientContext`] from a resolved timezone and OS.
///
/// Only `timezone` + `os` are ever populated — a browser cannot know the
/// account/device fields, so they stay absent. Blank values are dropped, and an
/// all-absent result is `None` so the SPA sends `client_context: None` rather
/// than an empty object.
pub fn browser_client_context(
    timezone: Option<String>,
    os: Option<String>,
) -> Option<ClientContext> {
    let ctx = ClientContext {
        timezone: clean(timezone),
        os: clean(os),
        ..ClientContext::default()
    };
    (!ctx.is_empty()).then_some(ctx)
}

/// Map a browser's `navigator.platform` + `navigator.userAgent` to a coarse OS
/// family (`"macOS"`, `"Windows"`, `"Linux"`, `"Android"`, `"iOS"`), falling
/// back to the raw platform string when unrecognised and `None` when both inputs
/// are blank.
///
/// Order matters: Android and iOS report Unix-like platforms/user-agents, so
/// they are matched before the generic macOS / Linux checks.
pub fn coarse_os(platform: &str, user_agent: &str) -> Option<String> {
    let p = platform.trim();
    let ua = user_agent;
    let family = if p.contains("Android") || ua.contains("Android") {
        "Android"
    } else if p.contains("iPhone")
        || p.contains("iPad")
        || p.contains("iPod")
        || ua.contains("iPhone")
        || ua.contains("iPad")
        || ua.contains("iPod")
    {
        "iOS"
    } else if p.starts_with("Mac") || ua.contains("Mac OS") || ua.contains("Macintosh") {
        "macOS"
    } else if p.starts_with("Win") || ua.contains("Windows") {
        "Windows"
    } else if p.contains("Linux") || ua.contains("Linux") || ua.contains("X11") {
        "Linux"
    } else if !p.is_empty() {
        // Unknown but non-empty: pass the raw platform through (still coarse).
        return Some(p.to_string());
    } else {
        return None;
    };
    Some(family.to_string())
}

// --- Browser glue + settings toggle (wasm only) ------------------------------

#[cfg(target_arch = "wasm32")]
pub use wasm::{
    load_persisted_share_device_info, resolve_browser_context, share_device_info_toggle,
};

#[cfg(target_arch = "wasm32")]
mod wasm {
    use leptos::prelude::*;
    use wasm_bindgen::JsValue;

    use super::{SHARE_DEVICE_INFO_DEFAULT, browser_client_context, coarse_os};
    use crate::engine::ViewSignals;
    use desktop_assistant_api_model::ClientContext;

    /// localStorage key for the per-device "Share device info" toggle.
    const TOGGLE_KEY: &str = "adele.share_device_info";

    /// Resolve the browser-scoped [`ClientContext`]: the user's IANA timezone and
    /// a coarse OS family — the only two fields a browser can honestly know
    /// (#557). `None` when neither resolves. This is called ONCE at engine
    /// construction; the resolved value is stamped on each turn while the toggle
    /// is on (the toggle is read at send time, not baked in here).
    pub fn resolve_browser_context() -> Option<ClientContext> {
        browser_client_context(resolve_timezone(), resolve_os())
    }

    /// The user's IANA timezone via `Intl.DateTimeFormat().resolvedOptions().timeZone`
    /// (e.g. `"Europe/London"`). `None` if the runtime can't report it.
    fn resolve_timezone() -> Option<String> {
        let dtf = js_sys::Intl::DateTimeFormat::new(&js_sys::Array::new(), &js_sys::Object::new());
        let resolved = dtf.resolved_options();
        js_sys::Reflect::get(resolved.as_ref(), &JsValue::from_str("timeZone"))
            .ok()?
            .as_string()
    }

    /// A coarse OS family from `navigator.platform` + `navigator.userAgent`. Both
    /// getters can throw in locked-down runtimes; a failure degrades to an empty
    /// input, which [`coarse_os`] resolves to `None`.
    fn resolve_os() -> Option<String> {
        let nav = web_sys::window()?.navigator();
        let platform = nav.platform().unwrap_or_default();
        let user_agent = nav.user_agent().unwrap_or_default();
        coarse_os(&platform, &user_agent)
    }

    /// Read the persisted "Share device info" toggle. Defaults to
    /// [`SHARE_DEVICE_INFO_DEFAULT`] (on) when unset — it is an opt-out.
    pub fn load_persisted_share_device_info() -> bool {
        use gloo_storage::{LocalStorage, Storage};
        LocalStorage::get::<bool>(TOGGLE_KEY).unwrap_or(SHARE_DEVICE_INFO_DEFAULT)
    }

    /// Persist the toggle so it survives reloads.
    fn persist_share_device_info(on: bool) {
        use gloo_storage::{LocalStorage, Storage};
        let _ = LocalStorage::set(TOGGLE_KEY, on);
    }

    /// The "Share device info" settings panel body: a full-width switch (default
    /// on) plus a plain-language note about exactly what is and isn't shared. The
    /// engine reads `view.share_device_info` at send time, so flipping it takes
    /// effect on the next message with no reconnect.
    pub fn share_device_info_toggle(view: ViewSignals) -> impl IntoView {
        let on_click = move |_| {
            let now = !view.share_device_info.get_untracked();
            view.share_device_info.set(now);
            persist_share_device_info(now);
        };
        view! {
            <section class="panel device-panel">
                <div class="panel-intro">
                    <p class="panel-summary">
                        "Share your time zone and operating system so replies can use your local time."
                    </p>
                    <p class="panel-note muted">
                        "Your name, username, home folder, and device hostname are never shared."
                    </p>
                </div>
                <div class="field">
                    <button
                        class="toggle-row"
                        role="switch"
                        class:active=move || view.share_device_info.get()
                        aria-checked=move || {
                            if view.share_device_info.get() { "true" } else { "false" }
                        }
                        on:click=on_click
                    >
                        <span class="toggle-label">"Share device info"</span>
                        <span class="toggle-state">
                            {move || if view.share_device_info.get() { "On" } else { "Off" }}
                        </span>
                    </button>
                </div>
            </section>
        }
    }
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
        assert_eq!(
            coarse_os("SomeFutureOS", "").as_deref(),
            Some("SomeFutureOS")
        );
    }

    #[test]
    fn coarse_os_empty_is_none() {
        assert_eq!(coarse_os("", ""), None);
    }
}
