# Agent Instructions — adele-web-ui

Repo-specific conventions for the mobile-first web client. Cross-project workflow rules (issue/PR/board sync, parallel worktrees, warnings-are-failures, security review posture, TDD posture) are embedded below under **Cross-project engineering standards**.

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

## Cross-project engineering standards

These apply to every repo under `github.com/adelie-ai`. They're embedded in each repo's `AGENTS.md` (not centralized) so a contributor working in a single repo has them in hand. Operator-specific preferences and machine-specific deploy recipes are intentionally not here.

### Don't break `main`
- `main` is the release: at any commit it must build, test, and run.
- Merge a green change as soon as it's independently shippable — additive, behavior-preserving, or behind a default that preserves the old path. Don't hold green work hostage to a coordinated release.
- Co-dependent changes land together; name the interlock ("blocked-by #X" / "must merge with #Y") so it's visible without reading the diff.
- "Green" is more than CI: review passed, tests cover the new behavior (not just "no panic"), warnings clean, security pass done, change stands on its own. With no active CI in these repos, "green" rests on local `cargo test` + `fmt` + `clippy --all-targets` + `cargo audit`, run by the author (via `just check` where the repo provides it).
- When in doubt, hold. A half-coupled "fix-forward" merge breaks `main` for everyone.

### Tests are spec-driven (TDD)
- Every change carries a Testing section: acceptance criteria as testable assertions, each criterion a named test whose name is legible from test output.
- Write failing tests first, in their own commit before the implementation commit — that commit is the spec.
- Cover all new code: every branch, error path, edge case. Gaps are a review finding.
- Assert the desired outcome, not just that a call returned `Ok`.
- Enumerate unhappy paths deliberately: empty/missing input, boundary/max, concurrent/racy, authorization/tenant boundaries, partial reads/writes/dropped streams, malformed input. A test list with none of these is testing wishes.

### Warnings are failures
- Compiler warnings, clippy lints, formatter diffs, and advisories all count — fix the root cause. If a lint truly doesn't apply, suppress at the narrowest scope with a one-line justification; never crate-wide.
- This repo enforces it **mechanically** via a `[lints]` table denying `rust.warnings` and `clippy.all`, so `cargo build`/`test`/`clippy` hard-fail on a warning — it isn't left to reviewer attention.
- Never `--no-verify` past hooks. If a hook is genuinely broken, fix it in its own commit and explain why.
- Don't `#[ignore]` a test you broke; fix it, or open a tracking issue and reference it from the attribute.
- Pre-existing warnings in a file you touch are yours to address (in-change or a small follow-up) — don't pile new code on an ignored signal.

### Security review before requesting review
- Read your own diff adversarially: untrusted input crossing trust boundaries (network, IPC, D-Bus, MCP tool args, **the browser**), secrets in logs, missing auth checks, panic-on-input, unparameterized SQL/shell.
- Scan dependencies whenever the lockfile changed (`cargo audit` or the `cve-mcp` server) — and scan BEFORE the first build, because build scripts execute attacker-controlled code at build time.
- High/critical CVEs are hard blockers: patch in the same change, prove the path unreachable and document why, or file a tracked follow-up referenced in the change. Never ship past one silently; never pin around an advisory without a comment or tracking issue.

### Maintainability / cognitive load
- Keep each change small enough to land independently with a clear deliverable.
- Don't introduce a new abstraction until ~3 call sites prove the pattern; when one new type unifies several needs, justify the unification explicitly.
- Reuse existing traits and patterns rather than inventing parallel ones; extend an existing crate over adding one unless the seam is obvious.

### Capability-based degradation
- Every reliance on an optional OS/desktop service (logind, screen-lock, KDE/Plasma, PipeWire specifics, any session- or system-bus D-Bus interface) must be capability-detected and degrade gracefully — never a hard dependency that errors or hangs when absent. The product may run headless, in containers, on other DEs, or as a system service.
- Distinguish "is the capability present?" from "did my call succeed?" Three states: absent → disable that feature, log once, fall back to prior behavior; present-and-known → use it; present-but-anomalous → stay conservative / last-known-state and warn. Scope any privacy/safety fail-safe to the last two — a fail-safe correct on the desktop can be pathological headless.
- Detect each optional dependency independently; absence of one never disables the others or aborts startup. Surface the detected capability so an operator sees *why* a feature is on or off.

### GitHub issue / PR / board hygiene
- Self-assign an issue when you start it (or comment to claim it) so parallel work doesn't collide; move the board card to In Progress.
- Link the PR to the issue: `Closes #N` to auto-close, `Refs #N` when it only partially addresses it.
- Keep the board in sync with reality (In Review on open, Done on merge); if you can't move the card, comment the intended status.
- On multi-session work, leave a short status comment before stopping — what landed, what's next, what's blocked — so state is reconstructable without git log.

### Worktrees
- Do code work in a git worktree on its own branch off `origin/main`, never the primary checkout, so concurrent sessions don't collide. Convention: `~/Projects/adelie-ai/.worktrees/<repo>/issue-N-slug/`, branch mirroring the slug.
- Run independent tasks in parallel worktrees, but check first for shared files / shared `Cargo.toml` dep edits / shared migration ordinals — if they overlap, serialize. Brief each parallel agent on its scope ("own crate X, don't touch Y").
