#!/bin/bash
# dev-admin.sh — rebuild control-plane, start API + admin React dev server.
# Always run via `just dev-admin` (not raw pnpm/vite or a stale binary).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
cd "$ROOT"

ADMIN_DIR="$ROOT/crates/control-plane/admin-ui"
PORT="${PORT:-3100}"
VITE_PORT="${VITE_ADMIN_PORT:-5174}"

if [[ -f "$ROOT/.env" ]]; then
  set -a
  # shellcheck disable=SC1091
  source "$ROOT/.env"
  set +a
fi
export PORT

free_port() {
  local p="$1"
  if lsof -ti ":$p" >/dev/null 2>&1; then
    echo "▶ stopping existing process on :$p..."
    lsof -ti ":$p" | xargs kill 2>/dev/null || true
    sleep 0.5
  fi
}

wait_for_health() {
  local pid="$1"
  for _ in $(seq 1 45); do
    if curl -sf "http://127.0.0.1:$PORT/v1/health" >/dev/null 2>&1; then
      return 0
    fi
    if ! kill -0 "$pid" 2>/dev/null; then
      echo "✗ control-plane exited before becoming healthy"
      return 1
    fi
    sleep 1
  done
  echo "✗ control-plane did not become healthy within 45s"
  return 1
}

cleanup() {
  if [[ -n "${CP_PID:-}" ]] && kill -0 "$CP_PID" 2>/dev/null; then
    kill "$CP_PID" 2>/dev/null || true
  fi
}
trap cleanup EXIT INT TERM

echo "▶ ensuring admin-ui deps..."
(cd "$ADMIN_DIR" && pnpm install --prefer-offline)

free_port "$PORT"

echo "▶ starting control-plane on :$PORT..."
"$ROOT/scripts/run-control-plane.sh" &
CP_PID=$!

wait_for_health "$CP_PID"

echo "✓ API ready     http://127.0.0.1:$PORT/v1/health"
echo "▶ starting admin UI (Vite proxies /v1 → :$PORT)..."
echo "  open http://localhost:$VITE_PORT/admin/"
echo ""

free_port "$VITE_PORT"
cd "$ADMIN_DIR"
exec pnpm dev -- --port "$VITE_PORT"
