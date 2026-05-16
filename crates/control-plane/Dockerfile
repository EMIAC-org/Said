# syntax=docker/dockerfile:1.7

FROM node:22-bookworm-slim AS admin-builder
WORKDIR /app/admin-ui

RUN corepack enable
COPY admin-ui/package.json admin-ui/pnpm-lock.yaml ./
RUN pnpm install --frozen-lockfile
COPY admin-ui/ ./
RUN pnpm run typecheck && pnpm run build

FROM rust:1-bookworm AS rust-builder
WORKDIR /app

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates pkg-config build-essential \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src admin-ui/dist/assets admin-ui/public admin \
    && printf 'fn main() {}' > src/main.rs \
    && printf '<!doctype html><html></html>' > admin-ui/dist/index.html \
    && printf '' > admin-ui/dist/assets/app.css \
    && printf '' > admin-ui/dist/assets/app.js \
    && printf '<!doctype html><html></html>' > admin/simulator.html \
    && printf '<!doctype html><html></html>' > admin-ui/public/preview-floors.html \
    && cargo build --release --locked \
    && rm -rf src admin-ui admin target/release/deps/control_plane*

COPY . .
COPY --from=admin-builder /app/admin-ui/dist ./admin-ui/dist
RUN cargo build --release --locked

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home --shell /usr/sbin/nologin control-plane

COPY --from=rust-builder /app/target/release/control-plane /usr/local/bin/control-plane

USER control-plane
ENV PORT=8080
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
    CMD curl -fsS "http://127.0.0.1:${PORT}/v1/health" >/dev/null || exit 1

CMD ["control-plane"]
