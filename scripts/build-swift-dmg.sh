#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

VERSION="${SAID_VERSION:-}"
if [ -z "$VERSION" ]; then
  VERSION=$(awk '
    /^\[workspace\.package\]/ { in_section = 1; next }
    /^\[/                     { in_section = 0 }
    in_section && /^[[:space:]]*version[[:space:]]*=/ {
      gsub(/.*=[[:space:]]*"/, "")
      gsub(/".*/, "")
      print
      exit
    }
  ' "$REPO_ROOT/Cargo.toml")
fi

[ -n "$VERSION" ] || { echo "could not parse Said version"; exit 1; }

ARCH="$(uname -m)"
case "$ARCH" in
  arm64) ARCH_SHORT="aarch64" ;;
  x86_64) ARCH_SHORT="x86_64" ;;
  *) ARCH_SHORT="$ARCH" ;;
esac

BUNDLE_DIR="$REPO_ROOT/target/swift-frontend/release/bundle"
APP_PATH="$BUNDLE_DIR/macos/Said.app"
DMG_DIR="$BUNDLE_DIR/dmg"
DMG_OUT="$DMG_DIR/Said_${VERSION}_${ARCH_SHORT}.dmg"
STAGING="$BUNDLE_DIR/dmg-staging"

echo "▶ Building said-backend sidecar"
cd "$REPO_ROOT"
cargo build -p said-backend --release

echo "▶ Building Swift app bundle"
SAID_VERSION="$VERSION" \
SAID_APP_DIR="$APP_PATH" \
SAID_BACKEND_PATH="$REPO_ROOT/target/release/said-backend" \
"$REPO_ROOT/swift-frontend/bundle.sh"

echo "▶ Building DMG"
rm -rf "$STAGING" "$DMG_OUT"
mkdir -p "$STAGING" "$DMG_DIR"
cp -R "$APP_PATH" "$STAGING/Said.app"
ln -s /Applications "$STAGING/Applications"

hdiutil create \
  -volname "Said" \
  -srcfolder "$STAGING" \
  -ov \
  -format UDZO \
  "$DMG_OUT" >/dev/null

rm -rf "$STAGING"
hdiutil verify "$DMG_OUT" >/dev/null

echo "✓ Swift DMG: $DMG_OUT"
