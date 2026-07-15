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

## `live_multi_client_sync.mjs`

Coverage for live multi-client sync (issue #15): the SPA reflecting activity in
OTHER clients (gtk/tui/kde/voice) with no manual refresh. A **stateful** fake BFF
speaks the real WS protocol and **pushes** server-initiated `WsFrame::Event`
frames the browser did not ask for, simulating another client. It asserts, in the
DOM: (1) on connect the SPA subscribes the open conversation (a
`subscribe_conversations` command carrying its id is observed); (2) a pushed
`user_message_added` + `assistant_delta` + `assistant_completed` for the open
conversation render the external turn live (user bubble → streaming → finalised
reply); (3) with the switcher drawer open, a pushed `conversation_title_changed`
renames a row in place and a `conversation_list_changed` (the fake BFF's list now
holding a new conversation) makes a new row appear — the reducer's refetch path;
and (4) after a simulated socket drop the SPA reconnects and **re-subscribes**
(a fresh `subscribe_conversations` for the open conversation), and a live event
pushed *after* the reconnect still renders. This proves the whole
event→`event_to_ui_message`→reducer→signals→DOM path in a real browser. The pure
`Event → UiMessage` mapping it relies on is unit-tested under `just check`
(`src/wire.rs`); the shared reducer's live-event handling lives in
`client-ui-common`. Run with `npm run test:live-sync`.

**Client scope:** this exercises the SPA's handling of pushed live events. The
real BFF (`crates/server`) blind-forwards the `SubscribeConversations` *command*
to the daemon, but its `ForwardingHandler` only relays a browser-initiated
send-turn's own events back — relaying the daemon's fanned-out *cross-client*
events to the browser is a separate `crates/server` follow-up. The client is
correct the moment those frames arrive, which is what this fake BFF proves.

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

## `global_personality_panel.mjs`

Browser check for the global personality panel (issue #17). Serves the built SPA
from a **stateful** fake BFF that holds a single global `Config` and mutates its
`personality` block on `set_config` (applying the `ConfigChanges`), returning the
config from both `get_config` and `set_config`. It drives the real client
round-trip in headless Chromium: open Settings → Global Personality, confirm the
seven traits pre-fill from the daemon's config (Expressive-7 defaults, every
trait a **concrete** level with exactly five options and **no** "Global
(inherit)" sentinel — unlike the per-conversation panel), change
`professionalism = Never` and `humor = Always`, **Save** (→ `SetConfig`), then
**reload the whole page** and assert the panel re-fills those two edits from the
stored config (`GetConfig`) while the untouched traits are unchanged — i.e. the
global change genuinely persists. Fails on any uncaught wasm panic.

The stateful fake keeps this deterministic and isolated from the shared local
daemon. The pure trait ⇄ config + `Personality` → `ConfigChanges` mapping it
renders is unit-tested under `just check` in `src/global_personality.rs`.

```sh
cd tests/e2e && npm run test:global-personality
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

## `read_aloud.mjs`

Browser check for read-aloud (issue #18). The browser's Web Speech API
(`window.speechSynthesis`) is **stubbed via `page.addInitScript`** before the SPA
loads — a browser API can only be observed by spying on it — recording every
`.speak(utterance.text)` and `.cancel()` on `window.__ra`. A minimal fake BFF
acks a sent message and streams the turn's `assistant_delta` + `assistant_completed`
so a reply genuinely completes. It drives the real client in headless Chromium and
asserts: (1) with the API present the toggle is shown and, while ON, a completed
reply is spoken with the reply's text; (2) toggling OFF mid-reply calls
`cancel()`, and a reply that completes while OFF is NOT spoken (the toggle gates
output, and the same reply is never double-spoken); (3) with `speechSynthesis`
stubbed **absent** the toggle is hidden and the app still sends/receives a turn
without error — capability detection degrades gracefully. Fails on any uncaught
wasm panic.

The pure decision core (enable / dedup / blank-skip / cancel) is unit-tested under
`just check` in `src/read_aloud.rs`; this covers only the browser SpeechSynthesis
+ reactive-DOM layer those host tests can't reach.

```sh
cd tests/e2e && npm run test:read-aloud
```

## `reauth_recovery.mjs`

Regression for graceful recovery from a rejected/expired session token (issue
#42). Before the fix the SPA held an invalid token and **retry-spammed** the
`/ws` upgrade forever ("WebSocket is already in CLOSING or CLOSED state",
"transport closed before reply") instead of recovering. It drives the built SPA
against three fake BFFs and asserts, in the DOM + storage + a console spy:

1. **Expired token** (seeded into `localStorage` via `page.addInitScript`, with an
   `exp` in the past): the app opens straight on the **login screen** (the
   pre-emptive `exp` check, never attempting a connect), the dead token is
   **cleared** from storage, and **no** CLOSING/CLOSED warning is logged. Logging
   in then connects and comes **online**.
2. **Rejected but un-expired token**: the fake BFF **refuses every `/ws` upgrade**
   (401, never opens) while the token's `exp` is still in the future. After a few
   fast failures the app drops to **login** and **stops retrying** — the harness
   counts `/ws` upgrade attempts and asserts they are bounded (≈3) and do not keep
   climbing — with the token cleared and **no** CLOSING/CLOSED spam.
3. **Healthy mid-session drop**: the BFF accepts, serves the initial load (the SPA
   goes online), then **drops the socket once**; the app **reconnects** (≥2
   connections observed) and stays in chat — it never drops to login, so the
   phone-sleep / network-change reconnect path is unregressed.

The pure logic it exercises — JWT `exp` classification and the reconnect /
auth-bail policy — is unit-tested under `just check` in `src/reauth.rs`; this
covers only the real-browser socket/storage/console behaviour those host tests
can't reach. Fails on any uncaught wasm panic.

```sh
cd tests/e2e && npm run test:reauth
```
