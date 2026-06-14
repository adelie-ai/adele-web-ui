set shell := ["bash", "-euo", "pipefail", "-c"]

default:
    @just --list

# --- Local verification ("local CI") -----------------------------------------
# We run these locally instead of GitHub Actions. `install-hooks` wires `check`
# into a git pre-push hook so it runs automatically before every push.

# Full local gate: formatting, lints, build, tests (on the pinned toolchain)
check: fmt-check lint build test

# Verify formatting without modifying files
fmt-check:
    cargo fmt --check

# Apply formatting
fmt:
    cargo fmt

# Clippy; warnings are errors
lint:
    cargo clippy --all-targets -- -D warnings

# Build the native workspace (the axum BFF server)
build:
    cargo build

# Run the test suite (excludes #[ignore] integration tests)
test:
    cargo test

# Build the Leptos wasm SPA (requires trunk + the wasm target). Enabled once
# crates/web lands (blocked on client-ui-common being wasm-clean). Until then
# this is a no-op so `just check` stays green on a server-only tree.
build-web:
    @if [ -d crates/web ]; then \
        command -v trunk >/dev/null || { echo "trunk not installed: cargo install trunk"; exit 1; }; \
        (cd crates/web && trunk build); \
    else echo "crates/web not present yet — skipping wasm SPA build"; fi

# Rebase onto latest origin/main then run the gate (catches clean-rebase-but-broken-build)
premerge:
    git fetch origin
    git rebase origin/main
    just check

# Install git hooks (pre-push runs `just check`). Local config; run once per clone.
install-hooks:
    git config core.hooksPath .githooks
    @echo "pre-push hook active — bypass once with: git push --no-verify"
