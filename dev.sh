#!/bin/bash
# dev.sh — build backend first, then launch Tauri dev mode.
# Always run this instead of `npm run tauri:dev` directly so the
# backend binary stays in sync with its source.
set -e
cd "$(dirname "$0")"

if [ -f ".env" ]; then
  set -a
  # shellcheck disable=SC1091
  source ".env"
  set +a
fi

# Codex/Electron-launched shells can inherit their host app's CoreFoundation
# bundle id. Force AirNote's identity for any helper process we spawn here.
export __CFBundleIdentifier=com.emiac.airnote.desktop
HOST_TARGET="$(rustc -vV | awk '/^host:/ {print $2}')"

echo "▶ building airnote-backend..."
touch crates/backend/src/main.rs
unset CARGO_TARGET_DIR
cargo build -p said-backend

echo "▶ syncing binary to Tauri externalBin..."
# Tauri copies binaries/airnote-backend-<target> into the build, overwriting
# target/debug/airnote-backend. Keep it in sync for the host triple.
cp target/debug/airnote-backend \
   "desktop/src-tauri/binaries/airnote-backend-$HOST_TARGET"

echo "▶ syncing whisper-cli to Tauri externalBin..."
if [ ! -x "target/$HOST_TARGET/release/whisper-cli" ]; then
  ./scripts/build-whisper-cli.sh "$HOST_TARGET"
fi
cp "target/$HOST_TARGET/release/whisper-cli" \
   "desktop/src-tauri/binaries/whisper-cli-$HOST_TARGET"

echo "▶ launching tauri dev..."
export AIRNOTE_DEV_STDERR=1
export RUST_LOG="${RUST_LOG:-info,said_desktop=debug,said_backend=debug,said_hotkey=debug,said_paster=debug}"
cd desktop
if [ "${AIRNOTE_DEV_APP_BUNDLE:-0}" = "1" ]; then
  RUNNER="$(cd .. && pwd)/scripts/tauri-dev-runner.sh"
  echo "▶ using macOS .app wrapper (logs may also be written to said.log/backend.log)"
  npm run tauri:dev -- --runner "$RUNNER"
else
  echo "▶ terminal logging enabled (RUST_LOG=$RUST_LOG)"
  npm run tauri:dev
fi
