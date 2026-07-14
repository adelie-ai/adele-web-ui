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

## `conversation_switcher.mjs`

Drives the conversation switcher (issue #12) in a real headless browser against a
**stateful** mock BFF that keeps an in-memory conversation list. It asserts, in
the DOM, that: the drawer lists the conversations with the open one marked;
tapping another row switches the chat (header + active marker update); "+ New
conversation" creates one and opens it; and deleting the one it created (via the
inline confirm) removes its row and re-homes the view to a remaining
conversation. Also fails on any uncaught wasm panic.

A stateful mock keeps this deterministic and isolated from the shared local
daemon (concurrent agents build against it) — the test never touches data it
didn't create. The pure row helpers (`src/sidebar.rs`) run under `just check`.

## `context_usage_indicator.mjs`

Coverage for the context-window usage indicator (issue #14). The fake BFF acks a
sent message and streams the turn's events back — including the per-turn
`context_usage` event (DA#341) — as correctly-nested `WsFrame::Event` frames.
The test asserts the indicator is **hidden** before any turn, then **appears**
after turn one with the shared `used / budget (pct%)` readout and the green
colour bucket, then **updates in place** to amber after a heavier second turn
crosses the 0.85 compaction line — proving the whole wire→reducer→engine→DOM
path in a real browser. The pure `used/budget/percent` formatting, colour
bucketing, and the web-specific `aria_label` / `bar_percent` are unit-tested
under `just check` (`client-ui-common`'s `context_usage` + `src/context.rs`);
this covers only the browser-render + reactive-update layer they can't reach.

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
