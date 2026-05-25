#!/bin/bash
# Build control-plane from source, then exec the debug binary.
# Always use this instead of invoking target/debug/control-plane directly.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CP_DIR="$ROOT/crates/control-plane"
BIN="$CP_DIR/target/debug/control-plane"

if [[ -f "$ROOT/.env" ]]; then
  set -a
  # shellcheck disable=SC1091
  source "$ROOT/.env"
  set +a
fi

export PORT="${PORT:-3100}"
export RUST_LOG="${RUST_LOG:-info}"

# Cursor/sandbox can set CARGO_TARGET_DIR to a cache outside the repo.
# Always build + run from this crate's local target/ so dev picks up latest code.
unset CARGO_TARGET_DIR
export CARGO_TARGET_DIR="$CP_DIR/target"

echo "▶ building control-plane..."
touch "$CP_DIR/src/lib.rs"
(cd "$CP_DIR" && cargo build --bin control-plane)

echo "▶ control-plane binary: $BIN"
exec "$BIN"
