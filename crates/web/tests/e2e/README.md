# Headless e2e regression tests

These exercise the **wasm SPA in a real headless browser** — the layer the pure
`cargo test` suites (`wire`, `model`, `reply`) can't reach. They are **explicitly
invoked**, never wired into `just check`, so the local gate stays browser-free.

## `transport_reliability.mjs`

Regression for the "model picker empty / Refresh does nothing" bug. It serves the
built SPA from a minimal fake BFF that speaks the real WS protocol but delivers
every reply as a **binary** WebSocket frame — exactly what a proxy/ingress can do
to a text payload.

Before the fix the read pump matched only `Message::Text` and silently dropped
`Message::Bytes`, so `list_available_models` never resolved, the sequential
initial load stalled on its first `await`, the connection never came online, and
the picker stayed empty with Refresh unable to recover. The test asserts the
connection comes **online** and the picker lists the chat-capable model. It also
fails on any uncaught wasm panic.

The companion unit tests in `src/reply.rs` (which *do* run under `just check`)
cover the per-request timeout — the general backstop for any reply that is never
delivered (stalled handler, lost/unparseable frame).

## Running

```sh
# 1. Build the SPA (produces crates/web/dist/, which the harness serves):
cd crates/web && trunk build

# 2. Install the harness deps + a headless Chromium (one-time):
cd tests/e2e && npm install && npx playwright install chromium

# 3. Run:
npm test
```

Exit code `0` = pass, `1` = assertion/panic failure, `2` = SPA not built.
