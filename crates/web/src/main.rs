//! adele-web-ui SPA entry point.
//!
//! On `wasm32` (the real build, via Trunk) this mounts the Leptos CSR app onto
//! `<body>`. On the host target it exists only so `cargo test` can exercise the
//! pure protocol modules without a browser runner: `main` is a no-op and the
//! leptos/gloo UI is `cfg`'d out. See `wire` for the host-testable logic.

#[cfg(target_arch = "wasm32")]
mod app;
#[cfg(target_arch = "wasm32")]
mod auth;
// The reducer-driving engine is host-testable (see its `#[cfg(test)]` module):
// it owns the view signals and dispatches `UiMessage`s through the shared
// reducer. On the host build its browser-only WebSocket transport is swapped for
// a tiny stub — the engine only names the `Transport` type and its
// `send_command`, and the host tests never open a connection.
#[cfg(any(target_arch = "wasm32", test))]
mod engine;
#[cfg(target_arch = "wasm32")]
mod settings;
#[cfg(target_arch = "wasm32")]
#[path = "transport.rs"]
mod transport;
#[cfg(all(not(target_arch = "wasm32"), test))]
#[path = "transport_stub.rs"]
mod transport;

// Pure, view-free modules consumed by the UI on wasm and unit-tested on the
// host: the wire-protocol mapping (`api::Event` -> `UiMessage`, frame
// round-trips), the model-selection helpers (issue #9), the connection form ⇄
// config mapping + credential logic (issue #10), the purposes slot/config
// mapping (issue #11), the personality trait ⇄ override mapping (issue #13),
// the global personality trait ⇄ config mapping (issue #17), and the
// transport's request/reply timeout core. Each
// pairs its pure logic with a `#[cfg(target_arch = "wasm32")]` Leptos view
// where it has one.
#[cfg(any(target_arch = "wasm32", test))]
mod connections;
#[cfg(any(target_arch = "wasm32", test))]
mod context;
// Conversation rename + archive/unarchive for the switcher (issue #49): the pure
// decision logic (host-tested here) plus its `#[cfg(target_arch = "wasm32")]`
// row-action / archived-section views over `sidebar`.
#[cfg(any(target_arch = "wasm32", test))]
mod conversation_manage;
#[cfg(any(target_arch = "wasm32", test))]
mod global_personality;
#[cfg(any(target_arch = "wasm32", test))]
mod knowledge;
// Chat markdown → sanitized HTML (issue #48): the pure `markdown_to_html`
// parse+sanitize core is host-tested here; a `#[cfg(target_arch = "wasm32")]`
// view sub-module sets the sanitized HTML in the Leptos chat bubbles.
#[cfg(any(target_arch = "wasm32", test))]
mod markdown;
// MCP-servers settings panel (issue #55): the pure `config_json` DTO mapping,
// status/transport display vocabulary, and env/args/scope parsers are
// host-tested here; a `#[cfg(target_arch = "wasm32")]` Leptos view renders the
// engine's `mcp_servers` signal. Pure additive client panel — no BFF change.
#[cfg(any(target_arch = "wasm32", test))]
mod mcp;
#[cfg(any(target_arch = "wasm32", test))]
mod model;
#[cfg(any(target_arch = "wasm32", test))]
mod personality;
#[cfg(any(target_arch = "wasm32", test))]
mod purposes;
// Message-queuing composer logic (feat/queue-messages): the pure chip-preview
// truncation + up/down recall-walk decisions are host-tested here; a
// `#[cfg(target_arch = "wasm32")]` Leptos view renders the queued-chips strip
// above the composer from the engine's `queued` signal. Queue *state* lives in
// the shared reducer — this only presents it.
#[cfg(any(target_arch = "wasm32", test))]
mod queue;
// Pure re-auth primitives (issue #42): JWT `exp` classification + the
// reconnect/auth-bail policy, host-tested here and consumed by `auth`/`app` on
// wasm.
#[cfg(any(target_arch = "wasm32", test))]
mod reauth;
// Browser read-aloud (issue #18): the pure decision core (host-testable) plus a
// `#[cfg(target_arch = "wasm32")]` `SpeechSynthesis` view — speaks completed
// assistant replies in-browser, no daemon change.
#[cfg(any(target_arch = "wasm32", test))]
mod read_aloud;
#[cfg(any(target_arch = "wasm32", test))]
mod reply;
#[cfg(any(target_arch = "wasm32", test))]
mod scratchpad;
#[cfg(any(target_arch = "wasm32", test))]
mod sidebar;
// Background-tasks panel (issue #50): pure formatting/list helpers host-tested
// here, plus a `#[cfg(target_arch = "wasm32")]` Leptos panel that renders the
// engine's live `tasks` signal.
#[cfg(any(target_arch = "wasm32", test))]
mod tasks;
// Opt-in "show tool activity" (#59): the merge-by-id + classify core is host-
// tested here; a `#[cfg(target_arch = "wasm32")]` collapsed `<details>` view
// renders the rows.
#[cfg(any(target_arch = "wasm32", test))]
mod tool_activity;
#[cfg(any(target_arch = "wasm32", test))]
mod wire;

fn main() {
    #[cfg(target_arch = "wasm32")]
    {
        console_error_panic_hook::set_once();
        leptos::mount::mount_to_body(app::App);
    }
}
