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

## Logging

`crates/server` reports traces, metrics and logs through the shared
[`adelie-telemetry`](https://github.com/adelie-ai/adelie-telemetry) crate. Console output
needs no collector and is on by default; exporting to an OpenTelemetry collector is
additional, behind the off-by-default `otel` Cargo feature (`Cargo.toml`).

- **Where logs go.** Always stderr, never stdout, at plain-text (not JSON). Set the
  filter with `RUST_LOG` (`RUST_LOG=debug ./adele-web-ui`); unset defaults to `info`.
- **The `otel` feature.** `cargo build --features otel` adds the OTLP exporters for
  traces, metrics and log records, configured entirely from the standard
  `OTEL_EXPORTER_OTLP_*` / `OTEL_TRACES_EXPORTER` / `OTEL_RESOURCE_ATTRIBUTES` and related
  variables - see `adelie-telemetry`'s own README for the full list. With the feature off,
  the BFF resolves no opentelemetry crate at all.
- **Spans.** One `http.request` span per inbound HTTP request, carrying `method`, `path`
  and (once known) `status`. One `daemon.call` span per command forwarded to the daemon,
  carrying only the command's name (`command`), never its arguments.
- **The level contract.** INFO carries ids, paths, status codes, command names, counts and
  durations - never content. DEBUG is the only level a prompt or a reply body may reach,
  because those are the "content" this service proxies (`RUST_LOG=debug` therefore means
  conversation content reaches wherever the logs go, deliberately).
- **Metrics.** Request duration by route and status, requests started/completed (an
  in-flight proxy - the shared metrics facade has counters and histograms, not gauges),
  and daemon-call duration and failure count by command. With `otel` off these still
  accumulate in process and print periodically (`adelie-telemetry`'s metrics summary).

## Status

Early development. See the issue tracker for the work breakdown.

## License

AGPL-3.0-or-later.
