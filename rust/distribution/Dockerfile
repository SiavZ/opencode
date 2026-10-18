# syntax=docker/dockerfile:1.7
# Purpose: production container for the Rust `opencode` server (`opencode serve`). Mirrors the
# intent of packages/containers (Ubuntu/Debian base, prebuilt toolchain, multi-arch via buildx)
# but ships only the runtime: a non-root user, a persistent data volume, a health check against
# the baseline `/global/health` route and tini-free signal handling (the binary is PID 1 via exec form).
# Scope: build context is `rust/` (see Dockerfile.dockerignore next to this file):
#   docker build -f rust/distribution/Dockerfile --platform linux/arm64 -t opencode-rust:dev rust
# Invariants: builds with --locked against the checked-in Cargo.lock, runs as uid 10001, writes
# only under /data (XDG dirs point there), never bakes credentials into a layer.

ARG RUST_IMAGE=rust:1.99-slim-trixie
ARG RUNTIME_IMAGE=debian:trixie-slim

FROM ${RUST_IMAGE} AS build
WORKDIR /src
RUN apt-get update \
  && apt-get install -y --no-install-recommends pkg-config ca-certificates \
  && rm -rf /var/lib/apt/lists/*
COPY . .
ARG PACKAGE=opencode
ARG BIN=opencode
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p "${PACKAGE}" --bin "${BIN}" \
  && install -Dm755 "target/release/${BIN}" /out/opencode

FROM ${RUNTIME_IMAGE} AS runtime
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates curl git ripgrep \
  && rm -rf /var/lib/apt/lists/* \
  && groupadd --system --gid 10001 opencode \
  && useradd --system --uid 10001 --gid opencode --home-dir /data --shell /usr/sbin/nologin opencode \
  && install -d -o opencode -g opencode -m 0750 /data /workspace
COPY --from=build /out/opencode /usr/local/bin/opencode

ENV HOME=/data \
    XDG_DATA_HOME=/data/share \
    XDG_CONFIG_HOME=/data/config \
    XDG_STATE_HOME=/data/state \
    XDG_CACHE_HOME=/data/cache \
    OPENCODE_PORT=4096
USER opencode:opencode
WORKDIR /workspace
VOLUME ["/data"]
EXPOSE 4096
STOPSIGNAL SIGTERM
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
  CMD curl -fsS "http://127.0.0.1:${OPENCODE_PORT}/global/health" || exit 1
ENTRYPOINT ["/usr/local/bin/opencode"]
CMD ["serve", "--hostname", "0.0.0.0", "--port", "4096"]
