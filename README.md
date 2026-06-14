# adele-web-ui

A mobile-first **web** client for the Adele desktop assistant, at feature parity with the
GTK and TUI clients. It is served by a small Rust/[axum](https://github.com/tokio-rs/axum)
backend-for-frontend (BFF); the UI itself is a [Leptos](https://leptos.dev) single-page app
compiled to WebAssembly.

> ## ⚠️ NOT FOR THE PUBLIC INTERNET
>
> **This service is not designed or hardened to be exposed to the internet.** Run it only
> on a private network you control, and reach it from your phone over a **VPN such as
> [Tailscale](https://tailscale.com) or WireGuard**. By default it binds to `127.0.0.1`;
> if you change the bind address, point it at your tunnel interface — never at a
> public address. You assume all risk if you ignore this.

This is a genuine web client for the mobile/remote case. It **complements** the fast native
GTK/TUI desktop clients — it does not replace them, and it is not a browser-wrapped desktop
app.

## Architecture

```
 Phone browser ──(Tailscale/WireGuard)── https/wss :9379 ┌──── adele-web-ui ────────────────────────┐
   Leptos SPA (wasm)  ◄── WsFrame/WsRequest (JSON) ─────► │  ws-interface router (reused from daemon)│
   gloo-net WebSocket                                     │   /ws  /login  /auth/config              │
                                                          │  ForwardingHandler ──UDS──► assistant     │
                                                          │  static SPA assets on /                   │
                                                          └───────────────────────────────────────────┘
```

- **Front door** (browser → BFF): the BFF embeds the assistant daemon's own `ws-interface`
  WebSocket server, so `/ws`, `/login` (JWT), and `/auth/config` are reused, not
  reimplemented.
- **Back door** (BFF → daemon): a single long-lived `client-common::Connector` over the
  local Unix socket, authenticated by peer-UID.
- **State logic**: the SPA reuses [`client-ui-common`](https://github.com/adelie-ai/client-ui-common)
  — the shared, transport-agnostic client core (`WindowState` reducer + `Effect`s) — so it
  behaves identically to the other clients.

## Configuration

Configured via TOML (also editable from the KDE System Settings module). Defaults:

| Key            | Default       | Notes                                            |
| -------------- | ------------- | ------------------------------------------------ |
| `enabled`      | `false`       | Whether the service runs at all                  |
| `bind_address` | `127.0.0.1`   | Set to your Tailscale/WireGuard interface IP     |
| `port`         | `9379`        | Listen port                                      |

## Status

Early development. See the issue tracker for the work breakdown.

## License

AGPL-3.0-or-later.
