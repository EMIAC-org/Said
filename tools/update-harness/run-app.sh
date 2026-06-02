#!/usr/bin/env bash
# Phase 2 — run the REAL app against the mock update server.
#
# No code change and no permanent config edit: we merge a dev-only updater
# override into tauri's config via `tauri dev --config`, pointing the app's
# updater at http://localhost:<port> with the TEST pubkey. The app's
# autoUpdate.ts then runs the real check → download → verify → "Update ready"
# pill against our mock.
#
# Limitation by design: `tauri dev` runs the binary directly (no bundled .app),
# so the actual install+relaunch SWAP can't happen here — this exercises
# check → download → verify → pill. The destructive full-swap is a separate
# `tauri build` + throwaway-copy test.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
PORT="${HARNESS_PORT:-3007}"
VERSION="${1:-99.0.0}"

# 1. keys + mock manifest
[[ -f "$HERE/keys/test.key" ]] || "$HERE/gen-keys.sh"
"$HERE/publish.sh" "$VERSION"

# 2. dev override config (merged over tauri.conf.json; arrays are replaced)
PUBKEY="$(tr -d '\n' < "$HERE/keys/test.key.pub")"
OVERRIDE="$HERE/mock/tauri.updater-test.json"
cat > "$OVERRIDE" <<JSON
{
  "plugins": {
    "updater": {
      "pubkey": "${PUBKEY}",
      "endpoints": ["http://localhost:${PORT}/latest.json"],
      "dangerousInsecureTransportProtocol": true
    }
  }
}
JSON
echo "✓ wrote dev override: $OVERRIDE"

# 3. serve mock in background; stop it when the app exits
python3 -m http.server "$PORT" --directory "$HERE/mock" >/tmp/harness-serve.log 2>&1 &
SRV=$!
trap 'kill $SRV 2>/dev/null || true; echo "stopped mock server"' EXIT
sleep 1
curl -fsS "http://localhost:${PORT}/latest.json" >/dev/null && echo "✓ mock server up on :$PORT (serving v$VERSION)"

# 4. backend sidecar in sync (same as dev.sh)
echo "▶ building airnote-backend…"
cd "$ROOT"
touch crates/backend/src/main.rs
unset CARGO_TARGET_DIR
cargo build -p said-backend
cp target/debug/airnote-backend \
   desktop/src-tauri/binaries/airnote-backend-aarch64-apple-darwin

# 5. launch dev app with the updater override merged in
echo "▶ launching app with updater pointed at localhost:$PORT …"
cd "$ROOT/desktop"
npm run tauri:dev -- --config "$OVERRIDE"
