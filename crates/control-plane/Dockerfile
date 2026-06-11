# syntax=docker/dockerfile:1.7
#
# Build context is the REPO ROOT (see deploy-control-plane.yml `context: .`).
# control-plane is excluded from the cargo workspace, but it path-depends on
# said-core (crates/core), which inherits `version.workspace` /
# `[workspace.dependencies]` from the repo-root Cargo.toml. So the whole
# workspace tree must be present for cargo to resolve said-core. A repo-root
# .dockerignore keeps the context lean (no target/, node_modules/, sidecar
# binaries, tools/).

FROM node:22-bookworm-slim AS admin-builder
WORKDIR /app/admin-ui

RUN corepack enable
COPY crates/control-plane/admin-ui/package.json crates/control-plane/admin-ui/pnpm-lock.yaml ./
RUN pnpm install --frozen-lockfile
COPY crates/control-plane/admin-ui/ ./
RUN pnpm run typecheck && pnpm run build

FROM rust:1-bookworm AS rust-builder
WORKDIR /app

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates pkg-config build-essential \
    && rm -rf /var/lib/apt/lists/*

# Full workspace tree (lean — see .dockerignore). Needed so said-core's
# workspace inheritance and the `../core` path dependency both resolve.
COPY . .
# dist/ is gitignored — overlay the admin UI built in the stage above so the
# control-plane binary's include_str!("../admin-ui/dist/...") finds it.
COPY --from=admin-builder /app/admin-ui/dist ./crates/control-plane/admin-ui/dist

WORKDIR /app/crates/control-plane
RUN cargo build --release --locked

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home --shell /usr/sbin/nologin control-plane

COPY --from=rust-builder /app/crates/control-plane/target/release/control-plane /usr/local/bin/control-plane

USER control-plane
ENV PORT=8080
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
    CMD curl -fsS "http://127.0.0.1:${PORT}/v1/health" >/dev/null || exit 1

CMD ["control-plane"]
