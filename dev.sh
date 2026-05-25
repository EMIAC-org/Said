#!/bin/bash
# dev.sh — build backend first, then launch Tauri dev mode.
# Always run this instead of `npm run tauri:dev` directly so the
# backend binary stays in sync with its source.
set -e
cd "$(dirname "$0")"

echo "▶ building said-backend..."
touch crates/backend/src/main.rs
unset CARGO_TARGET_DIR
cargo build -p said-backend

echo "▶ syncing binary to Tauri externalBin..."
# Tauri copies binaries/said-backend-aarch64-apple-darwin into the build,
# overwriting target/debug/said-backend. Keep them in sync.
cp target/debug/said-backend \
   desktop/src-tauri/binaries/said-backend-aarch64-apple-darwin

echo "▶ launching tauri dev..."
cd desktop
npm run tauri:dev
