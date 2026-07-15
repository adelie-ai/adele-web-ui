# Self-hosting adele-web-ui on Kubernetes

A complete, **environment-agnostic** guide to running the `adele-web-ui`
backend-for-frontend (BFF) + Leptos SPA on Kubernetes, reaching a
`desktop-assistant` daemon **over WebSocket** in the same cluster.

> ## ⚠️ NOT FOR THE PUBLIC INTERNET
>
> This service is **not hardened for internet exposure**. Run it on a private
> network you control and reach it from your phone over a **VPN (Tailscale /
> WireGuard)** or a **private/VPN-only ingress**. Everything below assumes a
> tailnet-only deployment. You assume all risk if you expose it publicly.

Every hostname, registry, cluster-issuer, username, IP, and namespace in this
guide is a **placeholder**. Substitute your own and keep the real values **out
of git** (render them in at apply time — recipes below). Placeholders used
throughout:

| Placeholder | Meaning |
| --- | --- |
| `registry.example.com:5000` | your image registry (the cluster can pull from it) |
| `adele-web-ui.example.com` | your internal / tailnet hostname for the UI |
| `your-clusterissuer` | your cert-manager `ClusterIssuer` |
| `<namespace>` | the namespace you deploy into |
| `<your-username>` | the browser front-door login username |
| `192.0.2.0/24` | any example IP range (TEST-NET-1) |

---

## 1. Architecture

```
 Phone browser ──(VPN: Tailscale/WireGuard)── https/wss :9379
        │
        ▼
 ┌──────────────────── adele-web-ui (BFF, axum) ─────────────────────┐
 │  FRONT DOOR (browser-facing): the daemon's own ws-interface server │
 │    /            → the Leptos wasm SPA (static assets, baked in)     │
 │    /login       → HTTP Basic → HS256 browser JWT                    │
 │    /ws          → JSON WsRequest/WsFrame (bearer-authenticated)     │
 │    /auth/config → auth parameters the SPA needs                     │
 │    /healthz     → readiness/liveness probe                          │
 │                                                                     │
 │  ForwardingHandler ── BACK DOOR (ws) ──► desktop-assistant daemon   │
 └─────────────────────────────────────────────────────────────────────┘
                              │  ws://adele-daemon:11339/ws  (in-cluster)
                              ▼
                   desktop-assistant daemon
```

- **One origin, two surfaces.** The BFF serves the SPA at `/` **and** proxies
  the browser to the daemon, so a browser that reaches the BFF gets both the UI
  and the API from a single origin. No separate API host, no CORS gymnastics
  beyond the `Origin` allowlist.
- **Front door** (browser → BFF): the BFF embeds `desktop-assistant`'s own
  `ws-interface` WebSocket server, so `/ws`, `/login`, and `/auth/config` are
  reused (not reimplemented), with an HS256 browser JWT the BFF mints and
  validates.
- **Back door** (BFF → daemon): a single long-lived `client-common::Connector`.
  Locally this is a Unix socket (peer-cred auth); **on Kubernetes it is a
  WebSocket** to the in-cluster daemon, authenticated by the daemon's `/login`
  password exchange.
- **Tailnet-only.** The daemon runs `ws://` (TLS off) inside the cluster; TLS is
  terminated at the ingress for the browser leg only, and the whole thing sits
  behind a VPN.

---

## 2. Prerequisites

- **A running `desktop-assistant` daemon, reachable in-cluster over WebSocket.**
  Deploy it first (see the `desktop-assistant` repo's `deploy/k8s/`). This guide
  assumes it is reachable at `ws://adele-daemon:11339/ws` — the Service name and
  port its manifests create. Adjust `ADELE_WEB_UI_DAEMON_WS_URL` if yours
  differ.
- **A shared `adele-secrets` Secret** (created by the daemon deploy) carrying a
  `WS_LOGIN_PASSWORD` key. The web UI reuses it for **both** the browser
  front-door password and the daemon back-door password, so there is one
  credential to manage. (§4 shows how to create it if the daemon deploy hasn't.)
- **A registry the cluster can pull from** (`registry.example.com:5000` below).
- **`kubectl`** pointed at the cluster, and a container builder (`podman` or
  `docker`) for the image.
- **cert-manager** in the cluster, only if you want the optional TLS ingress in
  §6.

### Namespace

The manifests in this directory declare an explicit `metadata.namespace`. Pick
**one** approach and use it consistently:

- **Edit the manifests** — change `metadata.namespace` in `50-web-ui.yaml` and
  `60-web-ui-ingress.yaml` to your `<namespace>` before applying; **or**
- **Strip the field** — remove the `namespace:` lines and pass `-n <namespace>`
  on every `kubectl apply`.

Because the manifests set `namespace:` explicitly, a bare `kubectl apply -f`
ignores any `-n` flag and lands in the manifest's namespace — so don't mix the
two. All example commands below use `-n <namespace>`; substitute your own.

---

## 3. Build the image

The image is built by the multi-stage `Dockerfile` at the repo root: stage 1
builds the Leptos wasm SPA with `trunk` and the axum BFF with `cargo`; stage 2
ships a slim glibc runtime with the BFF binary plus the built SPA assets baked
in at `/srv/web`.

### The build-context wrinkle (staged context)

The BFF path-deps `../desktop-assistant`, and the SPA (via `client-ui-common`)
path-deps `../desktop-assistant`, `../client-ui-common`, and `../voice`. Those
repos must sit as **siblings** of `adele-web-ui` inside the Docker build context
so the `../` paths resolve inside the container. The build context is therefore
a **staged directory** laid out exactly that way. Stage clean copies (no
`target/`, no `.git/`) with `rsync`:

```sh
# ADELE = your local adelie-ai checkout root (the dir holding the sibling repos).
ADELE=<path-to-your-adelie-ai-checkout>
CTX=$(mktemp -d)/webui-ctx
mkdir -p "$CTX"
for r in adele-web-ui desktop-assistant client-ui-common voice; do
  # Dereference symlinks (-L); drop build/vcs cruft so the context is small and
  # deterministic.
  rsync -aL --exclude target --exclude .git "$ADELE/$r/" "$CTX/$r/"
done
# If you are building from a git worktree, rsync THAT worktree in as
# adele-web-ui/ (it holds the Dockerfile + these manifests):
#   rsync -aL --exclude target --exclude .git <worktree>/ "$CTX/adele-web-ui/"
```

The `Dockerfile` `COPY`s `desktop-assistant/`, `client-ui-common/`, `voice/`,
then `adele-web-ui/` (last, so a source-only edit doesn't bust the heavier
sibling layers) — which is why the context root must contain all four as
siblings.

### Build → push

```sh
REG=registry.example.com:5000            # <-- your registry (do NOT commit)
TAG=web-ui-$(date +%Y%m%d)               # a descriptive, mutable tag
IMG="$REG/adele/adele-web-ui:$TAG"

# Build with the STAGED dir as context and this repo's Dockerfile.
podman build -t "$IMG" -f "$CTX/adele-web-ui/Dockerfile" "$CTX" \
  > /tmp/webui-build.log 2>&1
tail -1 /tmp/webui-build.log             # check the real exit line — do NOT pipe
                                         # `podman build` through tail directly
                                         # (it masks the exit code)
podman push "$IMG"                       # docker works identically
```

---

## 4. Secrets & ConfigMaps

Three objects hold the environment-specific values so they stay **out of the
repo** and a `kubectl apply` of the committed manifests can't revert them.
Create each once, in your `<namespace>`.

### `adele-secrets` — shared password (front + back door)

Usually created by the daemon deploy; the web UI reuses its `WS_LOGIN_PASSWORD`
for both the browser login **and** the daemon back-door login. Create it only if
the daemon deploy hasn't:

```sh
kubectl -n <namespace> create secret generic adele-secrets \
  --from-literal=WS_LOGIN_PASSWORD="$(openssl rand -base64 24)"
# The daemon's own deploy may also want POSTGRES_PASSWORD in this same Secret;
# that belongs to the daemon (the web UI does not use Postgres). Add it there if
# you are following the daemon guide:
#   --from-literal=POSTGRES_PASSWORD="$(openssl rand -base64 24)"
```

### `adele-web-ui-jwt` — **stable** HS256 signing key

The BFF signs the browser session JWTs with an HS256 key. **This key must be
stable across redeploys.** If it is left unset, the BFF generates a random key on
the per-pod `emptyDir` at startup — so **every rollout regenerates it**, which
invalidates every outstanding browser token and strands the SPA on a token the
new pod refuses. Pin it in a Secret and it survives redeploys:

```sh
kubectl -n <namespace> create secret generic adele-web-ui-jwt \
  --from-literal=ws_jwt_hs256_signing_key="$(openssl rand -hex 32)"
```

Rotating this key deliberately (recreate the Secret, restart the Deployment)
logs everyone out — that is the intended way to invalidate all sessions.

### `adele-web-ui-config` — env-specific login username

The browser login username is environment-specific, so it lives in a ConfigMap
(kept out of the repo) rather than hard-coded in `50-web-ui.yaml`. Sourcing it
from a ConfigMap also means a `kubectl apply` of the committed manifest can't
silently revert it to a default:

```sh
kubectl -n <namespace> create configmap adele-web-ui-config \
  --from-literal=login_username=<your-username>
```

---

## 5. Deploy the Deployment + Service

`50-web-ui.yaml` is the `Deployment` (runs non-root as UID 10001; an `emptyDir`
holds the fallback per-pod key) and the `Service` (`:9379`). It references all
three objects from §4 and carries a **placeholder** image
(`registry.example.com:5000/adele/adele-web-ui:REPLACE_ME`) — never commit your
real registry. Render the real image in at apply time:

```sh
# (Ensure the manifest's namespace matches <namespace> — see §2 Namespace.)
sed "s#registry.example.com:5000/adele/adele-web-ui:REPLACE_ME#$IMG#" \
  50-web-ui.yaml > /tmp/50-web-ui.rendered.yaml

kubectl apply -f /tmp/50-web-ui.rendered.yaml
kubectl -n <namespace> rollout status deploy/adele-web-ui
```

Key env in the Deployment (full reference in §8):

- `ADELE_WEB_UI_BIND_ADDRESS=0.0.0.0` / `ADELE_WEB_UI_PORT=9379` — bind inside
  the pod (the Service and probes target `9379`).
- `ADELE_WEB_UI_ALLOWED_ORIGINS` — the browser `Origin` allowlist. It ships with
  `http://localhost:9379` / `http://127.0.0.1:9379` (covering
  `kubectl port-forward`) plus the placeholder `https://adele-web-ui.example.com`
  for the ingress host. **Add every origin you actually reach the UI from** —
  the ingress host, and any Tailscale MagicDNS name if you connect over the
  tailnet without the ingress. An origin not in this list is rejected.
- `ADELE_WEB_UI_DAEMON_TRANSPORT=ws` + `ADELE_WEB_UI_DAEMON_WS_URL` — the WS back
  door to the in-cluster daemon.
- `ADELE_WEB_UI_LOGIN_USERNAME` ← ConfigMap `adele-web-ui-config`,
  `ADELE_WEB_UI_LOGIN_PASSWORD` and `ADELE_WEB_UI_DAEMON_WS_PASSWORD` ← Secret
  `adele-secrets`, `ADELE_WEB_UI_SIGNING_KEY` ← Secret `adele-web-ui-jwt`.

---

## 6. Ingress + TLS (optional)

Instead of `kubectl port-forward`, expose the UI on a hostname via the cluster's
ingress controller. `60-web-ui-ingress.yaml` is a cert-manager `Certificate`
(populates a TLS Secret from a `ClusterIssuer`) plus an `Ingress` — fully
placeholdered:

- `adele-web-ui.example.com` → your internal / tailnet host
- `your-clusterissuer` → your cert-manager `ClusterIssuer`

Render the real values in at apply time and re-apply the Deployment so its
`Origin` allowlist carries the real host:

```sh
HOST=adele-web-ui.example.com            # <-- your private/tailnet host (do NOT commit)
ISSUER=your-clusterissuer                # <-- your cert-manager ClusterIssuer (do NOT commit)

sed -e "s#adele-web-ui.example.com#$HOST#g" \
    -e "s#letsencrypt-example#$ISSUER#g" \
  60-web-ui-ingress.yaml > /tmp/60-web-ui-ingress.rendered.yaml
kubectl apply -f /tmp/60-web-ui-ingress.rendered.yaml

# Re-render the Deployment so the allowlist carries the real https origin.
sed -e "s#registry.example.com:5000/adele/adele-web-ui:REPLACE_ME#$IMG#" \
    -e "s#adele-web-ui.example.com#$HOST#g" \
  50-web-ui.yaml > /tmp/50-web-ui.rendered.yaml
kubectl apply -f /tmp/50-web-ui.rendered.yaml
```

**WebSocket upgrades:** the ingress controller must forward WebSocket upgrades to
`/ws`. Traefik (the `ingressClassName` in the shipped manifest) does this by
default; nginx and others may need an annotation or config to allow the
`Upgrade`/`Connection` headers through. The SPA derives `wss://` automatically
when served over TLS.

Keep this on a **private / VPN-only ingress**. The web UI is not for the public
internet.

---

## 7. Auth model

Two independent legs, one shared password:

1. **Browser → BFF (front door).** `POST /login` takes **HTTP Basic**
   credentials (`<your-username>` + the `WS_LOGIN_PASSWORD`) and, for a request
   whose `Origin` is in the allowlist, returns a signed **HS256 JWT**. The
   browser then opens `/ws` presenting that bearer. (Browsers can't set headers
   on a WebSocket, so the SPA passes the bearer via `Sec-WebSocket-Protocol`; a
   middleware relays it into the `Authorization` header the embedded ws-router
   validates. CLI clients can just send `Authorization: Bearer …` directly.)
   - **Signing key:** the stable HS256 key from `adele-web-ui-jwt` (§4). Stable
     ⇒ redeploys don't invalidate live logins.
   - **Token lifetime:** **7 days** by default
     (`DEFAULT_TOKEN_TTL_SECS = 604800`), overridable with
     `ADELE_WEB_UI_TOKEN_TTL_SECS`. (A short 15-minute TTL was tried and
     stranded the SPA on expiry; 7 days suits a single-user tailnet service.)
   - **Re-auth on expiry:** the SPA recovers gracefully. Before connecting it
     inspects the stored token's `exp` (no signature check) and, if it is past
     (within a small clock-skew margin), drops straight to the login screen
     instead of a doomed `/ws` upgrade. It also reacts to a rejected upgrade:
     after a few consecutive "closed-before-open" attempts with no working
     session between them (a rotated key, a revoked token), it clears the token
     and returns to login. A plain network stall never logs you out, so phones
     sleeping / changing networks keep reconnecting.
2. **BFF → daemon (back door).** The BFF authenticates to the in-cluster daemon
   with the daemon's own `/login` password exchange
   (`ADELE_WEB_UI_DAEMON_WS_USERNAME` / `_PASSWORD`), or a pre-minted daemon JWT
   (`ADELE_WEB_UI_DAEMON_WS_JWT`) if you have one. This is separate from the
   browser JWT above.

There is currently one static login password (PAM/system auth is a follow-up).
Because the same `WS_LOGIN_PASSWORD` backs both the browser login and the daemon
back door, there is a single credential to rotate.

---

## 8. Config / env reference

Every `ADELE_WEB_UI_*` variable the BFF reads (from
`crates/server/src/config.rs`). Env overlays the TOML config file and **wins**,
so a container needs no config file at all. Unset or blank values leave the
default untouched.

| Env var | Type | Default | Purpose |
| --- | --- | --- | --- |
| `ADELE_WEB_UI_ENABLED` | bool (`1`/`true`/`yes`/`on`) | `false` | Master switch. Must be truthy or the process logs and exits. The manifest sets `"true"`. |
| `ADELE_WEB_UI_BIND_ADDRESS` | string | `127.0.0.1` | Interface to bind. `0.0.0.0` in-pod (the Service fronts it); never a public address off-cluster. |
| `ADELE_WEB_UI_PORT` | u16 | `9379` | Listen port. |
| `ADELE_WEB_UI_ALLOWED_ORIGINS` | comma-separated list | *(empty)* | Browser `Origin` allowlist. **Empty rejects all browser clients**, so this must include every origin you reach the UI from. |
| `ADELE_WEB_UI_LOGIN_USERNAME` | string | `adele` | Username accepted at `POST /login`. Sourced from the `adele-web-ui-config` ConfigMap. |
| `ADELE_WEB_UI_LOGIN_PASSWORD` | string | *(none)* | Static `/login` password. Unset ⇒ `/login` issues no token (login disabled). From the `adele-secrets` Secret (`WS_LOGIN_PASSWORD`). |
| `ADELE_WEB_UI_DAEMON_TRANSPORT` | `ws` \| `uds` | `uds` | How the BFF reaches the daemon. **`ws` on Kubernetes.** Unknown values are ignored (keeps current). |
| `ADELE_WEB_UI_DAEMON_WS_URL` | string | *(none)* | Daemon WebSocket URL, e.g. `ws://adele-daemon:11339/ws`. **Required when transport = `ws`.** |
| `ADELE_WEB_UI_DAEMON_WS_USERNAME` | string | *(none)* | Username for the daemon's `/login` (WS back door). |
| `ADELE_WEB_UI_DAEMON_WS_PASSWORD` | string | *(none)* | Password for the daemon's `/login` (WS back door). From `adele-secrets`. |
| `ADELE_WEB_UI_DAEMON_WS_JWT` | string | *(none)* | Pre-minted daemon JWT. When set, used **instead of** the `/login` exchange. |
| `ADELE_WEB_UI_UDS_SOCKET` | path | platform default | Daemon Unix socket (only for `daemon_transport = uds`; unused on k8s). |
| `ADELE_WEB_UI_SIGNING_KEY` | string | per-process random on disk | HS256 key for the **browser** session tokens. Set it (from `adele-web-ui-jwt`) so it is **stable across restarts/redeploys** — otherwise every deploy invalidates all browser tokens. |
| `ADELE_WEB_UI_TOKEN_TTL_SECS` | u64 seconds | `604800` (7 days) | Browser session-token lifetime. Invalid values are ignored (keeps current). |
| `ADELE_WEB_UI_ISSUER` | string | local hostname | JWT `iss`. Self-consistent (the BFF issues and validates its own tokens); usually leave unset. |
| `ADELE_WEB_UI_AUDIENCE` | string | `<user>.adelie-ai` | JWT `aud`. Usually leave unset. |
| `ADELE_WEB_UI_WEB_DIR` | path | `crates/web/dist` (dev); `/srv/web` in-container | Directory of built SPA static assets served at `/`. Absent-on-disk is tolerated (API-only, logged). |

`RUST_LOG` (e.g. `info`) controls log verbosity as usual.

---

## 9. Verify / smoke test

```sh
kubectl -n <namespace> port-forward svc/adele-web-ui 9379:9379 &

# 1) Liveness/readiness endpoint.
curl -fsS http://127.0.0.1:9379/healthz && echo      # -> ok

# 2) The SPA is baked into the image, so `/` returns the app HTML.
curl -fsS -H 'Origin: http://localhost:9379' http://127.0.0.1:9379/ | head -c 200; echo

# 3) End-to-end round-trip through the BFF to the daemon.
PW=$(kubectl -n <namespace> get secret adele-secrets \
       -o jsonpath='{.data.WS_LOGIN_PASSWORD}' | base64 -d)

# POST /login is HTTP Basic (not JSON) + an allowed Origin -> browser JWT.
TOKEN=$(curl -fsS -u "<your-username>:$PW" -H 'Origin: http://localhost:9379' \
          -X POST http://127.0.0.1:9379/login \
        | python3 -c 'import sys,json;print(json.load(sys.stdin)["token"])')
echo "got a token of length ${#TOKEN}"

# Open /ws with the bearer (a CLI client can use the Authorization header;
# browsers pass it via Sec-WebSocket-Protocol), create_conversation, then
# send_message -> expect streamed assistant_delta / assistant_completed frames.
```

Then load `https://adele-web-ui.example.com` (or the port-forward URL) in a
browser over the VPN, log in with `<your-username>` + the password, and send a
message.

---

## 10. Troubleshooting

**Everyone is logged out after a redeploy.** The signing key is not stable.
Confirm the pod has `ADELE_WEB_UI_SIGNING_KEY` wired to the `adele-web-ui-jwt`
Secret, and that the Secret exists:

```sh
kubectl -n <namespace> get secret adele-web-ui-jwt
kubectl -n <namespace> get deploy adele-web-ui \
  -o jsonpath='{.spec.template.spec.containers[0].env[?(@.name=="ADELE_WEB_UI_SIGNING_KEY")]}'; echo
```

Without the Secret, the BFF falls back to a random per-pod key on the `emptyDir`
that regenerates on every rollout — the classic "logged out on deploy" symptom.

**`kubectl apply` reverted an env-specific value.** Any value hard-coded in the
committed manifest is overwritten by a re-apply. That is exactly why the login
username lives in the `adele-web-ui-config` ConfigMap and the passwords/key live
in Secrets — those are referenced by the manifest but not stored in it, so
`apply` can't clobber them. If you find yourself re-editing the manifest after
every apply, move that value into the ConfigMap/Secret (or your render `sed`)
instead. The image tag and the ingress host are handled by the render step (§5,
§6); don't commit them.

**Browser can't connect / `Origin` rejected.** The origin you're loading from
isn't in `ADELE_WEB_UI_ALLOWED_ORIGINS`. Add the exact scheme+host+port
(e.g. `https://adele-web-ui.example.com`, or your Tailscale MagicDNS origin) and
re-apply the Deployment.

**The `/ws` upgrade fails behind the ingress.** The ingress controller isn't
forwarding the WebSocket `Upgrade`. Traefik does by default; for other
controllers enable WebSocket/`Upgrade` passthrough on the route.

**Pod exits immediately with "disabled in config".** `ADELE_WEB_UI_ENABLED`
isn't truthy. It must be one of `1`/`true`/`yes`/`on`.

**BFF can't reach the daemon.** Check `ADELE_WEB_UI_DAEMON_WS_URL` resolves
in-cluster (default `ws://adele-daemon:11339/ws`), the daemon Service exists in
the same namespace, and the back-door credentials match the daemon's `/login`:

```sh
kubectl -n <namespace> logs deploy/adele-web-ui | grep -i daemon
kubectl -n <namespace> get svc adele-daemon
```

**No token from `/login`.** Either `ADELE_WEB_UI_LOGIN_PASSWORD` is unset (login
disabled — the logs warn `no login_password configured`), the Basic credentials
are wrong, or the request `Origin` isn't allowlisted.
