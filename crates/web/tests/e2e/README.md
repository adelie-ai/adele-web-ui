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

## `personality_panel.mjs`

Browser check for the per-conversation personality panel (issue #13). Serves the
built SPA from a **stateful** fake BFF that persists the last
`set_conversation_personality` per conversation and returns it from
`get_conversation`. It drives the real client round-trip in headless Chromium:
open Settings → Personality, confirm every trait starts on **Global**, pin
`humor = Never` and `directness = Always`, **Save** (→ `SetConversationPersonality`),
then **reload the whole page** and assert the panel pre-fills those two traits
from the stored override (`GetConversation` → `conversation_personality`) while
the rest still inherit — i.e. the override genuinely persists. Fails on any
uncaught wasm panic.

The stateful fake keeps this deterministic and isolated from the shared local
daemon. The pure trait ⇄ override mapping it renders is unit-tested under `just
check` in `src/personality.rs`.

```sh
cd tests/e2e && npm run test:personality
```

## `scratchpad_view.mjs`

Browser check for the read-only conversation scratchpad panel (issue #16). The
reducer fetches the active conversation's scratchpad on load and re-fetches
after every completed turn, so the **stateful** fake BFF answers
`get_conversation_scratchpad` with a note set that **changes** once a message is
sent. It drives the real client in headless Chromium: open Settings →
Scratchpad, assert the notes render grouped by type (a todo with an open
checkbox + a plain note) with a `2 notes · 0 of 1 done` summary; then send a
turn, reopen the panel, and assert it **updated in place** — the todo now struck
through/done, a newly-added todo present, and a `3 notes · 1 of 2 done` summary.
This proves the whole wire→reducer→engine→DOM refresh path. Fails on any
uncaught wasm panic.

The stateful fake keeps this deterministic and isolated from the shared local
daemon (it never touches data it didn't create). The pure grouping/labelling/
summary logic it renders is unit-tested under `just check` in `src/scratchpad.rs`.

```sh
cd tests/e2e && npm run test:scratchpad
```
