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
#[cfg(target_arch = "wasm32")]
mod engine;
#[cfg(target_arch = "wasm32")]
mod settings;
#[cfg(target_arch = "wasm32")]
mod transport;

// Pure, view-free modules consumed by the UI on wasm and unit-tested on the
// host: the wire-protocol mapping (`api::Event` -> `UiMessage`, frame
// round-trips), the model-selection helpers (issue #9), the connection form ⇄
// config mapping + credential logic (issue #10), the purposes slot/config
// mapping (issue #11), the personality trait ⇄ override mapping (issue #13),
// and the transport's request/reply timeout core. Each
// pairs its pure logic with a `#[cfg(target_arch = "wasm32")]` Leptos view
// where it has one.
#[cfg(any(target_arch = "wasm32", test))]
mod connections;
#[cfg(any(target_arch = "wasm32", test))]
mod context;
#[cfg(any(target_arch = "wasm32", test))]
mod model;
#[cfg(any(target_arch = "wasm32", test))]
mod personality;
#[cfg(any(target_arch = "wasm32", test))]
mod purposes;
#[cfg(any(target_arch = "wasm32", test))]
mod reply;
#[cfg(any(target_arch = "wasm32", test))]
mod scratchpad;
#[cfg(any(target_arch = "wasm32", test))]
mod sidebar;
#[cfg(any(target_arch = "wasm32", test))]
mod wire;

fn main() {
    #[cfg(target_arch = "wasm32")]
    {
        console_error_panic_hook::set_once();
        leptos::mount::mount_to_body(app::App);
    }
}
