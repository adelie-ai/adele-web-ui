# adele-web-ui on k8s (BFF → daemon over WebSocket)

Runs the `adele-web-ui` BFF (axum) in the same `adele-test` namespace as the
`desktop-assistant` daemon, reaching the daemon **over WebSocket** (`/login`
password auth) instead of the local Unix socket. The BFF also serves the Leptos
wasm SPA at `/`, so a browser reaching the BFF gets both the UI and the API from
one origin.

This assumes the daemon is already deployed (see the `desktop-assistant`
repo's `deploy/k8s/`) and the `adele-secrets` Secret exists with a
`WS_LOGIN_PASSWORD` key.

## Registry hygiene

The committed manifest uses a **placeholder** registry
(`registry.example.com:5000/adele/adele-web-ui:REPLACE_ME`). Never commit your
real registry hostname, cluster IPs, or node names. Render the real values into
a throwaway file at apply time (below), the same way the daemon deploy does.

## Build context wrinkle

The BFF path-deps `../desktop-assistant`, and the SPA (via `client-ui-common`)
path-deps `../desktop-assistant`, `../client-ui-common`, and `../voice`. Those
repos must be **siblings** of `adele-web-ui` inside the Docker build context so
the `../` paths resolve. Stage a clean context dir (no `target/`, no `.git/`):

```sh
# From anywhere; ADELE=your local adelie-ai checkout root.
ADELE=~/Projects/adelie-ai
CTX=$(mktemp -d)/webui-ctx
mkdir -p "$CTX"
for r in adele-web-ui desktop-assistant client-ui-common voice; do
  # Dereference symlinks; drop build/vcs cruft. Point adele-web-ui at THIS
  # worktree/checkout (the one holding the Dockerfile + manifests).
  rsync -aL --exclude target --exclude .git "$ADELE/$r/" "$CTX/$r/"
done
# (If building from a worktree, rsync the worktree in as adele-web-ui/.)
```

## Build → push → deploy

```sh
REG=registry.example.com:5000          # <-- your registry (do not commit)
TAG=ws-backdoor-$(date +%Y%m%d)        # a descriptive, mutable tag
IMG="$REG/adele/adele-web-ui:$TAG"

# Build with the STAGED dir as context and this repo's Dockerfile.
podman build -t "$IMG" -f "$CTX/adele-web-ui/Dockerfile" "$CTX" \
  > /tmp/webui-build.log 2>&1
tail -1 /tmp/webui-build.log            # check the real exit line — do NOT pipe
                                        # `podman build` through tail (masks the code)
podman push "$IMG"

# Render the placeholder → real image, then apply (creds never touched here).
sed "s#registry.example.com:5000/adele/adele-web-ui:REPLACE_ME#$IMG#" \
  deploy/k8s/50-web-ui.yaml > /tmp/50-web-ui.rendered.yaml
kubectl apply -f /tmp/50-web-ui.rendered.yaml
kubectl -n adele-test rollout status deploy/adele-web-ui
```

## Expose on a hostname (optional ingress)

Instead of `port-forward`, expose the UI via the cluster's ingress controller.
`60-web-ui-ingress.yaml` is a cert-manager `Certificate` + `Ingress`, fully
**placeholdered** — never commit your real host / cluster-issuer / node IPs.
Render them in at apply time and add the matching `https://<host>` browser
origin to the Deployment's allowlist (also placeholdered in `50-web-ui.yaml`):

```sh
HOST=your-host.example.internal        # <-- your private/tailnet host (do not commit)
ISSUER=your-clusterissuer              # <-- your cert-manager ClusterIssuer (do not commit)

sed -e "s#adele-web-ui.example.com#$HOST#g" \
    -e "s#letsencrypt-example#$ISSUER#g" \
  deploy/k8s/60-web-ui-ingress.yaml > /tmp/60-web-ui-ingress.rendered.yaml
kubectl apply -f /tmp/60-web-ui-ingress.rendered.yaml

# Re-render the Deployment so the allowlist carries the real origin, then apply.
sed -e "s#registry.example.com:5000/adele/adele-web-ui:REPLACE_ME#$IMG#" \
    -e "s#adele-web-ui.example.com#$HOST#g" \
  deploy/k8s/50-web-ui.yaml > /tmp/50-web-ui.rendered.yaml
kubectl apply -f /tmp/50-web-ui.rendered.yaml
```

Keep this on a private/VPN-only ingress; the web UI is not for the public
internet. The ingress controller must forward WebSocket upgrades to `/ws`
(Traefik does by default). The SPA derives `wss://` automatically under TLS.

## Smoke test

```sh
kubectl -n adele-test port-forward svc/adele-web-ui 9379:9379 &

# healthz
curl -fsS http://127.0.0.1:9379/healthz && echo

# End-to-end round-trip through the BFF to the daemon:
PW=$(kubectl -n adele-test get secret adele-secrets \
       -o jsonpath='{.data.WS_LOGIN_PASSWORD}' | base64 -d)

# 1) POST /login is HTTP Basic (not JSON) + an allowed Origin -> browser JWT.
TOKEN=$(curl -fsS -u "adele:$PW" -H 'Origin: http://localhost:9379' \
          -X POST http://127.0.0.1:9379/login | python3 -c 'import sys,json;print(json.load(sys.stdin)["token"])')

# 2) Open /ws with the bearer (a CLI client can use the Authorization header;
#    browsers pass it via Sec-WebSocket-Protocol), create_conversation, then
#    send_message -> expect streamed assistant_delta / assistant_completed.
#    (See the scratchpad ws test script used to verify this deploy.)

# 3) Bonus: the SPA is baked into the image, so `/` returns the app HTML.
curl -fsS -H 'Origin: http://localhost:9379' http://127.0.0.1:9379/ | head -c 200; echo
```
