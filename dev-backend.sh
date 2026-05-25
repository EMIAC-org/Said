#!/bin/bash
# dev-backend.sh — rebuild said-backend, then run the local daemon (no Tauri).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
cd "$ROOT"

BACKEND_PORT="${BACKEND_PORT:-48484}"

if [[ -f "$ROOT/.env" ]]; then
  set -a
  # shellcheck disable=SC1091
  source "$ROOT/.env"
  set +a
fi
export POLISH_SHARED_SECRET="${POLISH_SHARED_SECRET:-dev-secret}"

if lsof -ti ":$BACKEND_PORT" >/dev/null 2>&1; then
  echo "▶ stopping existing process on :$BACKEND_PORT..."
  lsof -ti ":$BACKEND_PORT" | xargs kill 2>/dev/null || true
  sleep 0.5
fi

echo "▶ building said-backend..."
touch crates/backend/src/main.rs
unset CARGO_TARGET_DIR
cargo build -p said-backend

echo "▶ starting said-backend on :$BACKEND_PORT..."
exec ./target/debug/said-backend --port "$BACKEND_PORT"
