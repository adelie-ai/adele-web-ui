# Multi-stage image for adele-web-ui: builds the Leptos wasm SPA (trunk) and the
# axum BFF (cargo), then ships a slim glibc runtime. glibc is matched to the
# daemon image (build on bookworm, run on bookworm-slim) so the shared
# desktop-assistant crates behave identically.
#
# BUILD CONTEXT WRINKLE: the BFF path-deps `../desktop-assistant`, and the SPA
# (via `client-ui-common`) path-deps `../desktop-assistant`, `../client-ui-common`
# and `../voice`. Those must sit as SIBLINGS of `adele-web-ui` so the `../`
# paths resolve inside the container. The build context is therefore a staged
# dir laid out exactly that way — see deploy/k8s/README.md for the helper that
# rsyncs clean copies (no target/, no .git/).

# ---- Builder ----------------------------------------------------------------
FROM rust:1-bookworm AS builder

# Prebuilt trunk (SPA bundler) — pinned; avoids a long `cargo install trunk`.
ARG TRUNK_VERSION=v0.21.14
RUN curl -sSfL \
      "https://github.com/trunk-rs/trunk/releases/download/${TRUNK_VERSION}/trunk-x86_64-unknown-linux-gnu.tar.gz" \
    | tar -xz -C /usr/local/bin trunk \
    && chmod +x /usr/local/bin/trunk

# Sibling repos, laid out so the `../` path-deps resolve. `adele-web-ui` last so
# a source-only edit doesn't bust the (heavier) sibling layers.
WORKDIR /build
COPY desktop-assistant/ ./desktop-assistant/
COPY client-ui-common/  ./client-ui-common/
COPY voice/             ./voice/
COPY adele-web-ui/      ./adele-web-ui/

# The pinned toolchain (rust-toolchain.toml at adele-web-ui) auto-provisions
# itself + the wasm32 target on first cargo/trunk invocation below.

# 1) Build the wasm SPA -> crates/web/dist. `crates/web` is its own workspace.
WORKDIR /build/adele-web-ui/crates/web
RUN trunk build --release

# 2) Build the BFF binary (native workspace).
WORKDIR /build/adele-web-ui
RUN cargo build --release --locked -p adele-web-ui-server

# ---- Runtime ----------------------------------------------------------------
FROM debian:bookworm-slim

# ca-certificates only — the BFF does NOT link libpam (unlike the daemon).
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN useradd --create-home --uid 10001 appuser
WORKDIR /home/appuser

# The BFF binary + the built SPA assets.
COPY --from=builder /build/adele-web-ui/target/release/adele-web-ui /usr/local/bin/adele-web-ui
COPY --from=builder /build/adele-web-ui/crates/web/dist/ /srv/web/

ENV RUST_LOG=info
ENV XDG_CONFIG_HOME=/home/appuser/.config
ENV XDG_DATA_HOME=/home/appuser/.local/share
# Serve the baked SPA by default (overridable via the same env at deploy time).
ENV ADELE_WEB_UI_WEB_DIR=/srv/web

EXPOSE 9379
USER appuser

ENTRYPOINT ["adele-web-ui"]
