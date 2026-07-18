# Deploying the web UI to Kubernetes

How to build the `adele-web-ui` image, push it to a registry your cluster can
pull from, and deploy one or more instances with kustomize.

This covers the **web client**: the axum backend-for-frontend (BFF) and the
Leptos wasm SPA it serves. It expects a `desktop-assistant` daemon already
running in the same namespace - see that repo's `docs/k8s-deployment.md` for the
brain, and deploy it first.

> ## NOT FOR THE PUBLIC INTERNET
>
> This service is not hardened for internet exposure. Run it on a private
> network and reach it over a VPN (Tailscale / WireGuard) or a VPN-only ingress.

> Every hostname, registry, namespace, and issuer below is a **placeholder**.
> This repo is public - real values belong in a private overlay, never in a
> commit. See [Private overlays](#private-overlays).

## Contents

- [Build and push](#build-and-push)
- [How the manifests are laid out](#how-the-manifests-are-laid-out)
- [The hostname appears in four places](#the-hostname-appears-in-four-places)
- [Deploy an instance](#deploy-an-instance)
- [Private overlays](#private-overlays)
- [Worked example: a second instance](#worked-example-a-second-instance)
- [Troubleshooting](#troubleshooting)

## Build and push

### The build-context wrinkle

The BFF path-depends on `../desktop-assistant`, and the SPA (via
`client-ui-common`) path-depends on `../desktop-assistant`,
`../client-ui-common`, and `../voice`. Those `../` paths must resolve *inside*
the container, so the build context is a **staged directory** with all four
repos as siblings - not this repo alone.

```sh
# ADELE = your checkout root, holding the sibling repos.
ADELE=<path-to-your-checkout-root>
CTX=$(mktemp -d)/web-ctx
mkdir -p "$CTX"

for repo in desktop-assistant client-ui-common voice adele-web-ui; do
  rsync -a --exclude target --exclude .git --exclude .worktrees \
        --exclude build --exclude .flatpak-builder --exclude .venv \
        "$ADELE/$repo/" "$CTX/$repo/"
done

podman build -t localhost/adele-web-ui:dev -f "$CTX/adele-web-ui/Dockerfile" "$CTX"
```

The exclude list matters. `target/` and `.git/` make the context enormous;
`build/`, `.flatpak-builder/`, and `.venv/` are packaging and tooling artifacts
that have no business in the image and have broken builds by shadowing real
paths.

The image builds the wasm SPA with a pinned `trunk`, builds the BFF with cargo,
and ships a `bookworm-slim` runtime with the SPA baked in at `/srv/web`.

### Tag and push

Tag with something **immutable and traceable** - a short commit SHA, optionally
prefixed with what changed. A moving tag like `latest` makes a rollout
unreproducible and a rollback guesswork.

```sh
REGISTRY=registry.example.com:5000
TAG=web-$(git -C "$ADELE/adele-web-ui" rev-parse --short HEAD)

podman tag localhost/adele-web-ui:dev "$REGISTRY/adele/adele-web-ui:$TAG"
podman push "$REGISTRY/adele/adele-web-ui:$TAG"
```

## How the manifests are laid out

`deploy/k8s/` is a kustomize base plus per-environment overlays:

```
deploy/k8s/
  base/                    namespace-agnostic; no hostname, no registry, no creds
    kustomization.yaml
    web-ui.yaml            Deployment + Service
    ingress.yaml           cert-manager Certificate + Ingress
  overlays/
    example/               the shape of an environment, with placeholder values
      kustomization.yaml
      host.yaml            ingress hostname + ClusterIssuer
      origins.yaml         browser Origin allowlist
  check-ingress-host.sh    asserts the hostname agrees in all four places
```

The base names no environment. An overlay supplies the namespace, the image tag,
and the hostname.

Render any overlay to see exactly what would be applied:

```sh
kubectl kustomize deploy/k8s/overlays/example
```

Validate offline, without touching a cluster:

```sh
just check-deploy
```

## The hostname appears in four places

This is the one thing that reliably goes wrong, so it gets its own section. A
single hostname must be spelled identically in:

1. `Certificate.spec.dnsNames` - else TLS is issued for the wrong name
2. `Ingress.spec.rules[].host` - else the router 404s
3. `Ingress.spec.tls[].hosts` - else no certificate is served
4. `ADELE_WEB_UI_ALLOWED_ORIGINS` - else the browser is rejected

Number 4 is the nasty one: TLS validates, routing works, the SPA loads, and then
every request fails an Origin check with nothing obviously wrong in the
infrastructure.

The example overlay splits these across `host.yaml` (1-3) and `origins.yaml`
(4), and `just check-deploy` renders the overlay and asserts they agree:

```
ingress host(s): adele.example.com
PASS ingress_host_matches_certificate_dnsnames
PASS ingress_host_matches_tls_hosts
PASS ingress_tls_uses_certificate_secret
PASS ingress_host_in_allowed_origins
```

Point it at a private overlay before applying that one:

```sh
ADELE_K8S_OVERLAY=~/deploy-env/production/web just check-deploy
```

Serving several hostnames at once is supported - most often while cutting a name
over between environments, when old and new must both resolve for a window. Each
host is held to all four checks, so a half-added name fails the same way a
half-changed one does.

## Deploy an instance

Namespace `adele-example` throughout; substitute your own. Deploy the daemon
first - the BFF's back door points at `ws://adele-daemon:11339/ws`.

### 1. Credentials and login username

The BFF reuses the daemon's `adele-secrets` for a single shared password, and
needs its own JWT signing key:

```sh
# Stable HS256 key. Without this the key regenerates on every deploy and
# strands every logged-in browser on an invalid token.
kubectl -n adele-example create secret generic adele-web-ui-jwt \
  --from-literal=ws_jwt_hs256_signing_key="$(openssl rand -hex 32)"

# Login username, from a ConfigMap so it stays out of git and an apply
# can't revert it to a committed default.
kubectl -n adele-example create configmap adele-web-ui-config \
  --from-literal=login_username=<your-username>
```

### 2. Apply

```sh
kubectl kustomize deploy/k8s/overlays/example | kubectl apply -f -
kubectl -n adele-example rollout status deploy/adele-web-ui
```

### 3. Wait for the certificate

```sh
kubectl -n adele-example get certificate adele-web-ui-cert -w
```

`READY=True` may take a minute or two. A DNS-01 solver additionally waits on DNS
propagation. If it stays `False`, read the CertificateRequest and Order events -
the message names the failing challenge.

### 4. Verify

```sh
curl https://adele.example.com/healthz          # -> ok
```

Then load it in a browser and log in. Before the ingress exists, or to isolate
the ingress from the app, port-forward instead:

```sh
kubectl -n adele-example port-forward svc/adele-web-ui 9379:9379
curl http://127.0.0.1:9379/healthz
```

`http://localhost:9379` is in the default allowlist precisely so this works.

## Private overlays

**This repo is public.** Real namespaces, registries, image tags, and hostnames
must not be committed. Keep a private overlay outside the repo and point it at
the in-repo base by relative path:

```
~/deploy-env/                       (private; not a git repo, or a private one)
  _bases/
    adele-web-ui -> symlink to <checkout>/adele-web-ui/deploy/k8s/base
  prod/
    web/
      kustomization.yaml
      host.yaml
      origins.yaml
```

```yaml
# ~/deploy-env/production/web/kustomization.yaml
apiVersion: kustomize.config.k8s.io/v1beta1
kind: Kustomization

namespace: adele-production

resources:
  - ../../_bases/adele-web-ui

images:
  - name: registry.example.com:5000/adele/adele-web-ui
    newName: registry.internal.example:5000/adele/adele-web-ui
    newTag: web-a1b2c3d

patches:
  - path: host.yaml
  - path: origins.yaml
```

A **symlinked base** keeps the private overlay independent of where the checkout
lives - repointing it after moving or merging is one `ln -sfn`. kustomize
follows it.

## Worked example: a second instance

Running a test and a production instance side by side. They share a cluster and
differ only in their overlays.

| | test | prod |
| --- | --- | --- |
| namespace | `adele-staging` | `adele-production` |
| hostname | `adele-staging.example.com` | `adele.example.com` |
| image tag | whatever is being tried | pinned, immutable |

Everything else is identical, and each namespace gets its own `adele-web-ui-jwt`
secret and `adele-web-ui-config` ConfigMap - they do not carry over.

### Moving a hostname between instances

Two Ingresses in different namespaces claiming the same host is undefined
behavior: whichever the controller resolves first wins, intermittently. Never
overlap. Instead, release the name before claiming it:

```sh
# 1. Add the new name to the OLD instance alongside the existing one, so it
#    serves both and nothing is down. Both go in host.yaml and origins.yaml.
kubectl kustomize ~/deploy-env/test/web | kubectl apply -f -

# 2. Verify the new name works, and wait for the cert to cover both names.
kubectl -n adele-staging get certificate adele-web-ui-cert
curl https://adele-staging.example.com/healthz

# 3. Stand the NEW instance up fully, but without its Ingress/Certificate.
#    Verify it end to end over a port-forward first.

# 4. Release the shared name from the old instance.
kubectl kustomize ~/deploy-env/test/web | kubectl apply -f -

# 5. Claim it on the new one.
kubectl kustomize ~/deploy-env/production/web | kubectl apply -f -
```

The window where the name is claimed by nobody is a few seconds, and at no point
is it claimed by two.

Note what this does to clients: anything still pointed at the moved hostname now
reaches the **new** instance, silently and with a valid certificate. Conversation
history does not follow a hostname - it lives in each namespace's Postgres.

## Troubleshooting

**Browser loads the SPA, then everything fails.** The host is missing from
`ADELE_WEB_UI_ALLOWED_ORIGINS` - place 4 of the four. `just check-deploy` catches
this before you apply.

**Everyone is logged out after a deploy.** `adele-web-ui-jwt` is missing, so the
signing key came from the per-pod emptyDir and regenerated. Create the Secret and
redeploy.

**Certificate stuck at `READY=False`.** Read the CertificateRequest, then the
Order and Challenge objects it owns - the failure message is on the challenge,
not the Certificate. For an internal hostname with private DNS, an HTTP-01
solver cannot work from the public internet; use a DNS-01 solver for that zone.

**502 / connection refused from the BFF.** The back door cannot reach the daemon.
Confirm `adele-daemon` Service exists in the same namespace and its pod is ready;
the BFF targets `ws://adele-daemon:11339/ws`.

**Login rejects the right-looking password.** The BFF validates against
`WS_LOGIN_PASSWORD` in `adele-secrets` and the username in the
`adele-web-ui-config` ConfigMap. On a second instance both are new - the values
from another namespace do not apply.
