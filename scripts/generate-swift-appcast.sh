#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RELEASE_DIR="${1:-$REPO_ROOT/target/swift-frontend/release/appcast-input}"
APPCAST_OUT="${APPCAST_OUT:-$REPO_ROOT/updater/appcast.xml}"
DOWNLOAD_URL_PREFIX="${DOWNLOAD_URL_PREFIX:-}"
RELEASE_LINK="${RELEASE_LINK:-https://github.com/EMIAC-org/Said/releases}"
SPARKLE_GENERATE_APPCAST="${SPARKLE_GENERATE_APPCAST:-generate_appcast}"

[ -d "$RELEASE_DIR" ] || { echo "release dir not found: $RELEASE_DIR"; exit 1; }
[ -n "${SPARKLE_PRIVATE_KEY:-}" ] || { echo "SPARKLE_PRIVATE_KEY is required"; exit 1; }
[ -n "$DOWNLOAD_URL_PREFIX" ] || { echo "DOWNLOAD_URL_PREFIX is required"; exit 1; }
command -v "$SPARKLE_GENERATE_APPCAST" >/dev/null 2>&1 || {
  echo "generate_appcast not found; set SPARKLE_GENERATE_APPCAST=/path/to/generate_appcast"
  exit 1
}

mkdir -p "$(dirname "$APPCAST_OUT")"

printf '%s' "$SPARKLE_PRIVATE_KEY" | "$SPARKLE_GENERATE_APPCAST" \
  --ed-key-file - \
  --link "$RELEASE_LINK" \
  --download-url-prefix "$DOWNLOAD_URL_PREFIX" \
  --embed-release-notes \
  -o "$APPCAST_OUT" \
  "$RELEASE_DIR"

echo "✓ Appcast: $APPCAST_OUT"
