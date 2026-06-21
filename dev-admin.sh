#!/bin/bash
# dev-admin.sh — rebuild control-plane, start API + admin React dev server.
# Always run via `just dev-admin` (not raw pnpm/vite or a stale binary).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
cd "$ROOT"

ADMIN_DIR="$ROOT/crates/control-plane/admin-ui"
PORT="${PORT:-3100}"
VITE_PORT="${VITE_ADMIN_PORT:-5174}"
DEV_DB_TUNNEL="${AIRNOTE_DEV_DB_TUNNEL:-auto}"
DEV_DB_SSH="${AIRNOTE_DEV_DB_SSH:-root@103.180.163.41}"
DEV_DB_LOCAL_PORT="${AIRNOTE_DEV_DB_LOCAL_PORT:-15433}"
DEV_DB_REMOTE_HOST="${AIRNOTE_DEV_DB_REMOTE_HOST:-127.0.0.1}"
DEV_DB_REMOTE_PORT="${AIRNOTE_DEV_DB_REMOTE_PORT:-5433}"

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

should_start_dev_db_tunnel() {
  if [[ "$DEV_DB_TUNNEL" == "0" || "$DEV_DB_TUNNEL" == "false" ]]; then
    return 1
  fi

  if [[ "$DEV_DB_TUNNEL" == "1" || "$DEV_DB_TUNNEL" == "true" ]]; then
    return 0
  fi

  [[ "${DATABASE_URL:-}" == *"127.0.0.1:${DEV_DB_LOCAL_PORT}"* || "${DATABASE_URL:-}" == *"localhost:${DEV_DB_LOCAL_PORT}"* ]]
}

start_dev_db_tunnel() {
  should_start_dev_db_tunnel || return 0

  if lsof -tiTCP:"$DEV_DB_LOCAL_PORT" -sTCP:LISTEN >/dev/null 2>&1; then
    echo "▶ using existing dev Postgres tunnel on :$DEV_DB_LOCAL_PORT..."
    return 0
  fi

  echo "▶ opening dev Postgres tunnel :$DEV_DB_LOCAL_PORT → $DEV_DB_SSH:$DEV_DB_REMOTE_PORT..."

  local password=""
  if [[ -n "${SSHPASS:-}" ]]; then
    password="$SSHPASS"
  elif command -v security >/dev/null 2>&1; then
    password="$(security find-generic-password -a root -s airnote-vm-root-password -w 2>/dev/null || true)"
  fi

  local ssh_args=(
    -f -N
    -o StrictHostKeyChecking=no
    -o ExitOnForwardFailure=yes
    -o ServerAliveInterval=30
    -L "127.0.0.1:${DEV_DB_LOCAL_PORT}:${DEV_DB_REMOTE_HOST}:${DEV_DB_REMOTE_PORT}"
    "$DEV_DB_SSH"
  )

  if [[ -n "$password" ]] && command -v sshpass >/dev/null 2>&1; then
    SSHPASS="$password" sshpass -e ssh "${ssh_args[@]}"
  else
    ssh "${ssh_args[@]}"
  fi

  sleep 0.5
  TUNNEL_PID="$(lsof -tiTCP:"$DEV_DB_LOCAL_PORT" -sTCP:LISTEN 2>/dev/null | head -n 1 || true)"
  if [[ -z "$TUNNEL_PID" ]]; then
    echo "✗ dev Postgres tunnel did not open on :$DEV_DB_LOCAL_PORT"
    return 1
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
  if [[ -n "${TUNNEL_PID:-}" ]] && kill -0 "$TUNNEL_PID" 2>/dev/null; then
    kill "$TUNNEL_PID" 2>/dev/null || true
  fi
}
trap cleanup EXIT INT TERM

echo "▶ ensuring admin-ui deps..."
(cd "$ADMIN_DIR" && pnpm install --prefer-offline)

free_port "$PORT"
start_dev_db_tunnel

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
export VITE_API_TARGET="${VITE_API_TARGET:-http://127.0.0.1:$PORT}"
echo "  API proxy target  $VITE_API_TARGET"
exec pnpm dev -- --port "$VITE_PORT"
