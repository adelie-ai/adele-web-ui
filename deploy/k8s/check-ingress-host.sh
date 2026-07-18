#!/usr/bin/env bash
# Named-check assertions for the web-UI ingress hostname coupling.
#
# The hostname appears in four coupled places (deploy/k8s/base/ingress.yaml
# documents them). Any one of them can be updated alone, and the instance then
# half-works in a way that is annoying to diagnose - most nastily the Origin
# allowlist, where TLS and routing look fine and only the browser is rejected.
# These checks render a kustomize overlay and assert the four agree, so drift
# fails the gate instead of the browser.
#
# Manifest-shape tests only: renders locally, never contacts the API server.
#
# Named checks (legible from output, one requirement each):
#   ingress_host_matches_certificate_dnsnames - Ingress host is on the cert
#   ingress_host_matches_tls_hosts            - Ingress host is in its own tls block
#   ingress_tls_uses_certificate_secret       - tls secretName == Certificate secretName
#   ingress_host_in_allowed_origins           - https://<host> is in the Origin allowlist
#
# Usage: check-ingress-host.sh [overlay-dir]   (default: the in-repo example)
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/../.." && pwd)"
overlay="${1:-${repo_root}/deploy/k8s/overlays/example}"

if [ ! -f "${overlay}/kustomization.yaml" ]; then
  echo "FAIL: no kustomization.yaml in overlay: ${overlay}" >&2
  exit 1
fi

echo "checking ingress-host coherence in: ${overlay}"

# Render to a temp file and pass its path as argv. NOT a pipe into `python3 -`:
# there the heredoc is itself stdin, so the piped render would be discarded and
# every check would vacuously "find 0 resources".
rendered="$(mktemp)"
trap 'rm -f "${rendered}"' EXIT
kubectl kustomize "${overlay}" > "${rendered}"

python3 - "${rendered}" <<'PY'
import sys

import yaml

with open(sys.argv[1]) as fh:
    docs = [d for d in yaml.safe_load_all(fh) if d]

failures = []


def check(name, ok, reason=""):
    if ok:
        print(f"PASS {name}")
    else:
        print(f"FAIL {name}: {reason}")
        failures.append(name)


def one(kind):
    found = [d for d in docs if d.get("kind") == kind]
    if len(found) != 1:
        print(f"FAIL: expected exactly one {kind} in the rendered overlay, found {len(found)}")
        sys.exit(1)
    return found[0]


ingress = one("Ingress")
cert = one("Certificate")
deployment = one("Deployment")

# Usually one host, but an instance legitimately serves several at once - most
# often while a hostname is being cut over between environments, when the old
# and new names must both resolve for a window. Every host is held to all four
# checks, so a half-added name fails the gate the same way a half-changed one does.
rule_hosts = [r["host"] for r in ingress["spec"].get("rules", []) if "host" in r]
if not rule_hosts:
    print("FAIL: the Ingress declares no rule host")
    sys.exit(1)
print(f"ingress host(s): {', '.join(rule_hosts)}")

cert_names = cert["spec"].get("dnsNames", [])
tls_blocks = ingress["spec"].get("tls", [])
tls_hosts = [h for t in tls_blocks for h in t.get("hosts", [])]

containers = deployment["spec"]["template"]["spec"]["containers"]
origins_raw = next(
    (
        e.get("value", "")
        for c in containers
        for e in c.get("env", [])
        if e.get("name") == "ADELE_WEB_UI_ALLOWED_ORIGINS"
    ),
    None,
)

# --- ingress_host_matches_certificate_dnsnames -------------------------------
missing_from_cert = [h for h in rule_hosts if h not in cert_names]
check(
    "ingress_host_matches_certificate_dnsnames",
    not missing_from_cert,
    f"Certificate dnsNames {cert_names} do not include {missing_from_cert}; "
    "TLS would be issued for the wrong name",
)

# --- ingress_host_matches_tls_hosts ------------------------------------------
missing_from_tls = [h for h in rule_hosts if h not in tls_hosts]
check(
    "ingress_host_matches_tls_hosts",
    not missing_from_tls,
    f"Ingress tls hosts {tls_hosts} do not include {missing_from_tls}; "
    "the router would serve no certificate for them",
)

# --- ingress_tls_uses_certificate_secret -------------------------------------
cert_secret = cert["spec"].get("secretName")
tls_secrets = {t.get("secretName") for t in tls_blocks}
check(
    "ingress_tls_uses_certificate_secret",
    tls_secrets == {cert_secret},
    f"Ingress tls secretName(s) {sorted(s for s in tls_secrets if s)} must be exactly "
    f"the Certificate's secretName {cert_secret!r}, or the Ingress serves a "
    "certificate nothing populates",
)

# --- ingress_host_in_allowed_origins -----------------------------------------
if origins_raw is None:
    check(
        "ingress_host_in_allowed_origins",
        False,
        "the web-ui container sets no ADELE_WEB_UI_ALLOWED_ORIGINS env var",
    )
else:
    origins = [o.strip() for o in origins_raw.split(",") if o.strip()]
    missing_origins = [h for h in rule_hosts if f"https://{h}" not in origins]
    check(
        "ingress_host_in_allowed_origins",
        not missing_origins,
        f"allowed origins {origins} are missing "
        f"{[f'https://{h}' for h in missing_origins]}, so the browser is rejected "
        "on the Origin check after TLS and routing already succeeded",
    )

if failures:
    print(f"\n{len(failures)} check(s) failed: {', '.join(failures)}", file=sys.stderr)
    sys.exit(1)
print("\nAll ingress-host checks passed.")
PY
