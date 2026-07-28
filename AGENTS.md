# Agent Instructions — adele-web-ui

Shared standards live in [AGENTS.base.md](AGENTS.base.md), which is generated. This file holds the rules specific to this repo.

Repo-specific conventions for the mobile-first web client. The overrides and additions to the base are listed at the end of this file.

## What this repo is

A **mobile-first web client** for the Adele assistant, at feature parity with the GTK and TUI clients. Two parts:

1. **`crates/server`** — a small [axum](https://github.com/tokio-rs/axum) **backend-for-frontend (BFF)**. It embeds `desktop-assistant`'s own `ws-interface` WebSocket server as the browser-facing front door (reusing `/ws`, `/login`, `/auth/config` + JWT, not reimplementing them), and connects to the local daemon over **UDS** via `client-common`'s `Connector` as the back door. The only substantial new logic is a `ForwardingHandler: AssistantApiHandler` that bridges the two. The BFF forces `share_client_context = false` on that back-door connection (`daemon_conn.rs`): `client-common`'s native `resolve_client_context` reads the *server's* home/username/hostname/timezone/OS, which is the wrong machine for a browser user (#557), so it is never sent. A browser-scoped context — just the timezone and a coarse platform a browser can actually know — is resolved in the wasm client and attached separately (Refs #549/#557).
2. **`crates/web`** — a [Leptos](https://leptos.dev) single-page app compiled to `wasm32-unknown-unknown` (built with `trunk`). It reuses [`client-ui-common`](https://github.com/adelie-ai/client-ui-common) — the shared, transport-agnostic client core (`WindowState` reducer + `Effect`s + view-models) — so it behaves identically to the other clients. *(Lands once `client-ui-common` is wasm-clean; see the desktop-assistant protocol-crate work and `client-ui-common#1`.)*

> **NOT for the public internet.** This service is not hardened for internet exposure. It binds to `127.0.0.1` by default and is meant to be reached from a phone over a VPN (Tailscale/WireGuard). It **complements** the fast native GTK/TUI clients — it is not a browser-wrapped desktop app.

## Where things live

- `crates/server/src/main.rs` — entry point: config, bind address, axum router assembly, graceful shutdown.
- `crates/server/src/` — `ForwardingHandler`, the embedded `ws-interface` wiring, auth wiring (`WsBasicLogin`/`auth-jwt`), static-asset serving, config loading. One module per concern.
- `crates/web/src/` — the Leptos SPA: a thin `gloo-net` WebSocket transport, the `client-ui-common`-driven app state, and per-screen components mirroring gtk/tui (chat, sidebar, model/purpose/personality pickers, KB, tasks, settings).

## Web / Leptos conventions

- **Reducer-driven, not ad-hoc state.** UI state flows through `client-ui-common`'s `WindowState::apply(msg) -> Vec<Effect>`. Incoming wire `Event`s map to `UiMessage`s; the SPA executes returned `Effect`s (RPCs back over the WebSocket). Don't grow a parallel state machine — extend the shared core (in its repo) when something is missing.
- **One transport module.** All daemon I/O goes through the single `gloo-net` WebSocket client speaking `WsRequest`/`WsFrame` JSON from `desktop-assistant-protocol`. Correlate request→result by `id`. Reconnect + re-`SubscribeConversations` on resume (phones sleep and change networks).
- **Mobile-first.** Design for a phone viewport first; touch targets, responsive layout, no hover-only affordances.
- **Components mirror the other clients.** When a piece of UI grows past ~50 lines, give it its own component module and match the shape of the existing screens — and the gtk/tui equivalents, so parity is auditable.

## Shared types & version pinning

`desktop-assistant-protocol`, `api-model`, `client-common`, `ws-interface`, `auth-jwt`, and `client-ui-common` come from their repos (git deps; `Cargo.lock` pins the revision). When the daemon's protocol changes, bumping here is a deliberate update — coordinate the bump across web / TUI / GTK / KDE so the clients track the protocol together, and mention the corresponding daemon PR in the commit message.

## Build & install

- `cargo build`, `cargo test` — the native BFF server.
- `just build-web` — the wasm SPA (needs `cargo install trunk` + the `wasm32-unknown-unknown` target).
- `just check` — the full local gate (fmt, clippy, build, test). `just install-hooks` wires it into a pre-push hook (run once per clone).

## Dependency safety

This client is **network- and browser-facing** — a larger trust boundary than the native clients. Treat every byte from the browser as untrusted (validate the JWT on the `/ws` upgrade, enforce the `Origin` allowlist, never trust client-supplied identity). The SPA renders assistant-produced markdown — sanitize/escape on the render path. Scan the lockfile (`cargo audit` / `cve-mcp`) on every dependency change, including the wasm/JS-interop crates.

## Overrides and additions to the shared base

Everything in [AGENTS.base.md](AGENTS.base.md) applies to this repo. This section
records only the points where this repo deliberately differs from the base, or adds a
rule the base does not have.

### 3.1 The gate for this repo (addition)

The `adelie-ai` repos have no CI. The gate is local and the author runs it: `just check`.
Run `just install-hooks` once per clone to put the same gate on pre-push. Warnings are
denied mechanically by the workspace `[lints]` table, which every member crate inherits
with `[lints] workspace = true`, so `cargo build`, `cargo test`, and `cargo clippy` each
hard-fail on a warning.

### 4.3 Branch and pull request - merge when green (override, weaker than the base)

The base opens a pull request and waits for the user. In these repos the merge is delegated:
merge your own pull request as soon as it is green and independently shippable. Green here
means more than a clean build. The gate above passed, the tests cover the new behavior and
not only the absence of a panic, the security pass is done, and the change stands on its own.
Assign `dspadea` with `gh pr edit --add-assignee` and verify it; a review request from the
same account no-ops without an error, so never report a pull request as review-requested.
When in doubt, hold.

### 4.4 Worktrees - the group convention (addition)

Put the worktree at `.worktrees/<repo>/issue-N-slug/` under the group directory, on a branch
that mirrors the slug. Before you run tasks in parallel worktrees, look for shared files,
shared `Cargo.toml` dependency edits, and shared migration ordinals. Serialize the work where
they overlap, and tell each parallel agent the scope it owns.

### 5.1 Input safety - the browser is a trust boundary (addition)

Treat every byte that arrives from the browser as untrusted input, alongside the network,
IPC, D-Bus, and MCP tool arguments. Validate the JWT on the `/ws` upgrade, enforce the
`Origin` allowlist, and never trust a client-supplied identity. The single-page app renders
assistant-produced markdown, so sanitize and escape it on the render path.

### 6.1 Dependencies - a high or critical advisory is a hard blocker (override, stricter than the base)

Scan after you add a dependency and before the first build:

1. Add the dependency (`cargo add <crate>`). This writes the lockfile but does not build.
2. Scan the updated lockfile with the `cve-mcp` server's `scan_packages` tool, or with
   `cargo audit`. Pass every (name, version, ecosystem) tuple.
3. A high or critical finding blocks the change. Patch it in the same change, or prove the
   path unreachable and write down why, or file an issue and reference it from the change.
4. Build only after the scan is clean, or after you have accepted the findings in writing.

Never pin around an advisory without a comment or a tracked issue.

### 9.1 Tracker for this project

GitHub Issues on `github.com/adelie-ai/adele-web-ui`, together with the shared `adelie-ai` project
board. Manage entries with the `gh` CLI (`gh issue create`, `gh issue list`, `gh issue edit`,
`gh pr create`). The board states in use are In Progress, In Review, and Done.

### Capability-based degradation (addition)

Every reliance on an optional operating-system or desktop service - logind, the screen lock,
KDE and Plasma, PipeWire specifics, any session-bus or system-bus D-Bus interface - must be
capability-detected, and must degrade cleanly when the service is absent. Never make one a
hard dependency that errors or hangs. The product can run headless, in a container, on
another desktop environment, or as a system service.

Distinguish "is the capability present?" from "did my call succeed?". There are three states.
Absent: disable that feature, log once, and fall back to the prior behavior. Present and
known: use it. Present but anomalous: stay conservative, or hold the last known state, and
warn. Scope any privacy or safety fail-safe to the last two states only. A fail-safe that is
correct on the desktop can be pathological headless. "Treat an unknown session as inactive"
means the microphone never opens.

Detect each optional dependency on its own. The absence of one never disables the others and
never aborts startup. Surface the detected capability, so an operator can see why a feature
is on or off.
