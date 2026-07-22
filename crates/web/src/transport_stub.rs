//! Host-build stand-in for the browser WebSocket [`Transport`](crate::transport).
//!
//! The real transport (`transport.rs`) is `gloo-net`/`web-sys` based and only
//! meaningful in the browser, so it stays wasm-only. The reducer-driving
//! [`Engine`](crate::engine::Engine) is host-testable, though, and names the
//! `Transport` type (its `transport: Option<Rc<Transport>>` field) plus
//! `send_command` in its RPC methods. This stub supplies exactly that surface so
//! `engine` compiles off wasm; the host tests never establish a connection (they
//! leave the engine's transport `None`, so every RPC method early-returns and no
//! `send_command` here is ever awaited).

use desktop_assistant_api_model::{Command, CommandResult};

/// A do-nothing transport for the host test build. Constructed only in tests
/// that want to exercise the connected paths; the queue tests leave the engine's
/// transport unset, so this is never actually driven.
pub struct Transport;

impl Transport {
    /// Signature-compatible with the real transport's `send_command`. Never
    /// reached in the host tests (the engine's transport stays `None`); returns
    /// an error so any accidental call fails loudly rather than hanging.
    pub async fn send_command(&self, _command: Command) -> Result<CommandResult, String> {
        Err("transport stub: send_command is not available on the host build".to_string())
    }
}
