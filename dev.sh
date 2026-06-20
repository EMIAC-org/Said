#!/bin/bash
# dev.sh — build backend first, then launch Tauri dev mode.
# Always run this instead of `npm run tauri:dev` directly so the
# backend binary stays in sync with its source.
set -e
cd "$(dirname "$0")"

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
RUNNER="$(pwd)/scripts/tauri-dev-runner.sh"
cd desktop
npm run tauri:dev -- --runner "$RUNNER"
