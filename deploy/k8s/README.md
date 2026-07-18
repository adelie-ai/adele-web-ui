# adele-web-ui Kubernetes manifests

A kustomize base plus per-environment overlays for the `adele-web-ui`
backend-for-frontend (BFF) and the Leptos wasm SPA it serves, reaching a
`desktop-assistant` daemon over WebSocket in the same namespace.

> ## NOT FOR THE PUBLIC INTERNET
>
> This service is **not hardened for internet exposure**. Run it on a private
> network you control and reach it from your phone over a **VPN (Tailscale /
> WireGuard)** or a **private/VPN-only ingress**. Everything here assumes a
> VPN-only deployment. You assume all risk if you expose it publicly.

**The deployment guide is [`../../docs/k8s-deployment.md`](../../docs/k8s-deployment.md)** -
building and pushing the image, deploying an instance step by step, private
overlays, running a second instance, moving a hostname between instances, and
troubleshooting. This file documents the manifests themselves: what they
contain, what they expect to already exist, and how they are validated.

This repo is **public**. Every hostname, registry, namespace, issuer, and
username in these files is a placeholder. Real values belong in a private
overlay outside the repo, never in a commit.

## Layout

```
deploy/k8s/
  base/                    namespace-agnostic; no hostname, no registry, no creds
    kustomization.yaml
    web-ui.yaml            Deployment + Service
    ingress.yaml           cert-manager Certificate + Ingress
  overlays/
    example/               the shape of an environment, with placeholder values
      kustomization.yaml   namespace + image (registry and immutable tag)
      host.yaml            Certificate dnsNames, Ingress rules/tls host, ClusterIssuer
      origins.yaml         browser Origin allowlist
  check-ingress-host.sh    asserts the hostname agrees in all four places
```

The base names no environment: it sets no `namespace`, and carries a placeholder
image (`registry.example.com:5000/adele/adele-web-ui:replace-me`). An overlay
supplies the namespace, the real registry and tag, and the hostname - which is
what lets one base render several instances into different namespaces.

Render an overlay to see exactly what would be applied:

```sh
kubectl kustomize deploy/k8s/overlays/example
```

## Architecture

```
  Phone browser --(VPN: Tailscale/WireGuard)--> https/wss :9379
        |
        v
  +---------------- adele-web-ui (BFF, axum) ------------------+
  | FRONT DOOR (browser-facing): the daemon's ws-interface      |
  |   /            -> the Leptos wasm SPA (baked into the image) |
  |   /login       -> HTTP Basic -> HS256 browser JWT            |
  |   /ws          -> JSON WsRequest/WsFrame (bearer-auth)       |
  |   /auth/config -> auth parameters the SPA needs              |
  |   /healthz     -> readiness/liveness probe                   |
  |                                                              |
  | ForwardingHandler -- BACK DOOR (ws) --> desktop-assistant     |
  +--------------------------------------------------------------+
                       |  ws://adele-daemon:11339/ws  (in-cluster)
                       v
             desktop-assistant daemon
```

- **One origin, two surfaces.** The BFF serves the SPA at `/` **and** proxies the
  browser to the daemon, so a browser that reaches the BFF gets both the UI and
  the API from a single origin. No separate API host, no CORS gymnastics beyond
  the `Origin` allowlist.
- **Front door** (browser -> BFF): the BFF embeds `desktop-assistant`'s own
  `ws-interface` WebSocket server, so `/ws`, `/login`, and `/auth/config` are
  reused rather than reimplemented, with an HS256 browser JWT the BFF mints and
  validates.
- **Back door** (BFF -> daemon): a single long-lived `client-common::Connector`.
  Locally that is a Unix socket with peer-cred auth; in-cluster it is a
  WebSocket, authenticated by the daemon's own `/login` password exchange.
- **VPN-only.** The daemon speaks plain `ws://` inside the cluster; TLS is
  terminated at the ingress for the browser leg only, and the whole thing sits
  behind a VPN.

Because browsers cannot set headers on a WebSocket, the SPA passes its bearer
token via `Sec-WebSocket-Protocol` and a middleware relays it into the
`Authorization` header the embedded ws-router validates. CLI clients can send
`Authorization: Bearer ...` directly.

## What the base expects to already exist

Credentials and environment-specific identity are referenced by the Deployment
but never stored in it, so they stay out of git and a `kubectl apply` of the
committed manifests cannot revert them. Create all three once per namespace,
imperatively.

| Object | Kind | Key | Holds |
| --- | --- | --- | --- |
| `adele-secrets` | Secret | `WS_LOGIN_PASSWORD` | the one shared password: browser front door **and** daemon back door |
| `adele-web-ui-jwt` | Secret | `ws_jwt_hs256_signing_key` | stable HS256 key for browser session tokens |
| `adele-web-ui-config` | ConfigMap | `login_username` | the username accepted at `POST /login` |

`adele-secrets` is normally created by the daemon deploy; the web UI reuses it so
there is a single credential to rotate. Create it here only if the daemon deploy
has not:

```sh
kubectl -n <namespace> create secret generic adele-secrets \
  --from-literal=WS_LOGIN_PASSWORD="$(openssl rand -base64 24)"

kubectl -n <namespace> create secret generic adele-web-ui-jwt \
  --from-literal=ws_jwt_hs256_signing_key="$(openssl rand -hex 32)"

kubectl -n <namespace> create configmap adele-web-ui-config \
  --from-literal=login_username=<your-username>
```

**The signing key must be stable across redeploys.** Left unset, the BFF
generates a random key on the per-pod `emptyDir` at startup, so every rollout
regenerates it, invalidates every outstanding browser token, and strands the SPA
on a token the new pod refuses. Rotating the Secret deliberately and restarting
the Deployment is the supported way to log everyone out.

## The hostname appears in four coupled places

A single hostname must be spelled identically in all four, or the instance
half-works in a way that is annoying to diagnose:

1. `Certificate.spec.dnsNames` - else TLS is issued for the wrong name
2. `Ingress.spec.rules[].host` - else the router 404s
3. `Ingress.spec.tls[].hosts` - else no certificate is served
4. `ADELE_WEB_UI_ALLOWED_ORIGINS` - else the browser is rejected

Number 4 is the nasty one: TLS validates, routing works, the SPA loads, and then
every request fails an `Origin` check with nothing obviously wrong in the
infrastructure. The example overlay splits these across `host.yaml` (1-3) and
`origins.yaml` (4).

Serving several hostnames at once is supported - most often while cutting a name
over between environments, when old and new must both resolve for a window. Each
host is held to all four checks, so a half-added name fails the same way a
half-changed one does.

## Validation

```sh
just check-deploy
```

This renders both the base and the example overlay, schema-validates them
client-side, and runs `check-ingress-host.sh` against the target overlay. It
never contacts the API server, so it is safe in CI and against a cluster you are
not currently pointed at.

Point it at a private overlay before applying that one:

```sh
ADELE_K8S_OVERLAY=~/deploy-env/production/web just check-deploy
```

`check-ingress-host.sh` emits one named check per requirement, so a failure names
the unmet requirement rather than a line number:

```
ingress host(s): adele.example.com
PASS ingress_host_matches_certificate_dnsnames
PASS ingress_host_matches_tls_hosts
PASS ingress_tls_uses_certificate_secret
PASS ingress_host_in_allowed_origins
```

## Config / env reference

Every `ADELE_WEB_UI_*` variable the BFF reads (from
`crates/server/src/config.rs`). Env overlays the TOML config file and **wins**,
so a container needs no config file at all. Unset or blank values leave the
default untouched.

| Env var | Type | Default | Purpose |
| --- | --- | --- | --- |
| `ADELE_WEB_UI_ENABLED` | bool (`1`/`true`/`yes`/`on`) | `false` | Master switch. Must be truthy or the process logs and exits. The base sets `"true"`. |
| `ADELE_WEB_UI_BIND_ADDRESS` | string | `127.0.0.1` | Interface to bind. `0.0.0.0` in-pod (the Service fronts it); never a public address off-cluster. |
| `ADELE_WEB_UI_PORT` | u16 | `9379` | Listen port. The Service and both probes target `9379`. |
| `ADELE_WEB_UI_ALLOWED_ORIGINS` | comma-separated list | *(empty)* | Browser `Origin` allowlist. **Empty rejects all browser clients**, so it must include every origin you reach the UI from - the ingress host, plus any Tailscale MagicDNS name if you connect over the tailnet directly. Patched per environment by `origins.yaml`. |
| `ADELE_WEB_UI_LOGIN_USERNAME` | string | `adele` | Username accepted at `POST /login`. From the `adele-web-ui-config` ConfigMap. |
| `ADELE_WEB_UI_LOGIN_PASSWORD` | string | *(none)* | Static `/login` password. Unset means `/login` issues no token (login disabled). From `adele-secrets`. |
| `ADELE_WEB_UI_DAEMON_TRANSPORT` | `ws` \| `uds` | `uds` | How the BFF reaches the daemon. **`ws` on Kubernetes.** Unknown values are ignored (keeps current). |
| `ADELE_WEB_UI_DAEMON_WS_URL` | string | *(none)* | Daemon WebSocket URL, e.g. `ws://adele-daemon:11339/ws`. **Required when transport is `ws`.** |
| `ADELE_WEB_UI_DAEMON_WS_USERNAME` | string | *(none)* | Username for the daemon's `/login` (back door). |
| `ADELE_WEB_UI_DAEMON_WS_PASSWORD` | string | *(none)* | Password for the daemon's `/login` (back door). From `adele-secrets`. |
| `ADELE_WEB_UI_DAEMON_WS_JWT` | string | *(none)* | Pre-minted daemon JWT. When set, used **instead of** the `/login` exchange. |
| `ADELE_WEB_UI_UDS_SOCKET` | path | platform default | Daemon Unix socket. Only for `uds` transport; unused in-cluster. |
| `ADELE_WEB_UI_SIGNING_KEY` | string | per-process random on disk | HS256 key for **browser** session tokens. From `adele-web-ui-jwt`, so it survives redeploys. |
| `ADELE_WEB_UI_TOKEN_TTL_SECS` | u64 seconds | `604800` (7 days) | Browser session-token lifetime. Invalid values are ignored (keeps current). A short TTL strands the SPA on expiry; 7 days suits a single-user VPN service. |
| `ADELE_WEB_UI_ISSUER` | string | local hostname | JWT `iss`. Self-consistent, since the BFF issues and validates its own tokens; usually leave unset. |
| `ADELE_WEB_UI_AUDIENCE` | string | `<user>.adelie-ai` | JWT `aud`. Usually leave unset. |
| `ADELE_WEB_UI_WEB_DIR` | path | `crates/web/dist` (dev); `/srv/web` in-container | Directory of built SPA assets served at `/`. Absent-on-disk is tolerated (API-only, logged). |

`RUST_LOG` (for example `info`) controls log verbosity as usual.

## Session expiry behaviour

The SPA recovers from an expired or rejected token on its own. Before connecting
it inspects the stored token's `exp` without checking the signature and, if it is
past a small clock-skew margin, drops straight to the login screen instead of
attempting a doomed `/ws` upgrade. It also reacts to rejected upgrades: after a
few consecutive closed-before-open attempts with no working session in between -
a rotated key, a revoked token - it clears the token and returns to login. A
plain network stall never logs you out, so phones that sleep or change networks
keep reconnecting.

There is one static login password; PAM/system auth is a tracked follow-up.

## Ingress notes

The ingress controller must forward WebSocket upgrades to `/ws`. Traefik, the
`ingressClassName` in the base, does this by default; nginx and others may need
an annotation or route setting to let the `Upgrade`/`Connection` headers through.
The SPA derives `wss://` automatically when it is served over TLS.

Keep this on a private, VPN-only ingress.
