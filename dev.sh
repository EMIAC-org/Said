#!/bin/bash
# dev.sh — build backend first, then launch Tauri dev mode.
# Always run this instead of `npm run tauri:dev` directly so the
# backend binary stays in sync with its source.
set -e
cd "$(dirname "$0")"

echo "▶ building airnote-backend..."
touch crates/backend/src/main.rs
unset CARGO_TARGET_DIR
cargo build -p said-backend

echo "▶ syncing binary to Tauri externalBin..."
# Tauri copies binaries/airnote-backend-aarch64-apple-darwin into the build,
# overwriting target/debug/airnote-backend. Keep them in sync.
cp target/debug/airnote-backend \
   desktop/src-tauri/binaries/airnote-backend-aarch64-apple-darwin

echo "▶ launching tauri dev..."
cd desktop
npm run tauri:dev
