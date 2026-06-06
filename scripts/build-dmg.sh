#!/bin/bash
# Build a release DMG of AirNote.app with a stable ad-hoc signature.
#
# The ad-hoc signature is what lets macOS TCC track the app by bundle ID
# instead of binary hash, so granted permissions (Input Monitoring,
# Accessibility, Microphone) survive future rebuilds. Without it, every
# `tauri build` produces a new cdhash and TCC silently drops the grant.
#
# This wrapper also pre-cleans the read-write DMG state that Tauri's
# bundle_dmg.sh routinely leaves attached after a previous run, which
# is the cause of the recurring "failed to run bundle_dmg.sh" error.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DESKTOP_DIR="$REPO_ROOT/desktop"
TAURI_DIR="$DESKTOP_DIR/src-tauri"

# Target triple — first arg, or default to the host arch (aarch64 on Apple
# Silicon dev machines). CI passes both targets explicitly via the matrix.
TARGET="${1:-aarch64-apple-darwin}"
case "$TARGET" in
  aarch64-apple-darwin) ARCH_SHORT="aarch64" ;;
  x86_64-apple-darwin)  ARCH_SHORT="x86_64"  ;;
  *) echo "unsupported target: $TARGET (expected aarch64-apple-darwin or x86_64-apple-darwin)" >&2; exit 1 ;;
esac

BUNDLE_DIR="$REPO_ROOT/target/$TARGET/release/bundle"
APP_PATH="$BUNDLE_DIR/macos/AirNote.app"
SIDECAR_SRC="$REPO_ROOT/target/$TARGET/release/airnote-backend"
SIDECAR_DEST="$TAURI_DIR/binaries/airnote-backend-$TARGET"
BUNDLE_ID="com.emiac.airnote.desktop"

# Read the workspace version from Cargo.toml. Single source of truth — bumped
# via scripts/bump-version.sh, never hand-edited here.
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
[ -n "$VERSION" ] || { echo "could not parse [workspace.package].version from Cargo.toml"; exit 1; }

bold='\033[1m'; green='\033[0;32m'; yellow='\033[1;33m'; red='\033[0;31m'; nc='\033[0m'
step()  { echo -e "\n${bold}▶ $*${nc}"; }
ok()    { echo -e "  ${green}✓ $*${nc}"; }
warn()  { echo -e "  ${yellow}⚠ $*${nc}"; }
fail()  { echo -e "\n  ${red}✗ $*${nc}\n"; exit 1; }

export PATH="$HOME/.cargo/bin:$PATH"

# ── Pre-clean: undo whatever Tauri's bundle_dmg.sh left attached ─────────────
step "Pre-clean: detach stale AirNote volumes & temp DMGs"

# Detach any volume mounted with the product name (read-only finalized DMGs
# the user mounted, plus any read-write working DMG still attached from a
# prior failed run).
for vol in "/Volumes/AirNote" "/Volumes/AirNote 1" "/Volumes/AirNote 2"; do
  if mount | grep -q "on $vol "; then
    hdiutil detach "$vol" -force 2>/dev/null || true
    ok "detached $vol"
  fi
done

# Detach any read-write working image Tauri left attached. Those have the
# tell-tale name rw.<pid>.<product>_<version>_<arch>.dmg
while IFS= read -r dev; do
  [ -n "$dev" ] || continue
  hdiutil detach "$dev" -force 2>/dev/null || true
  ok "detached $dev (stale rw image)"
done < <(hdiutil info | awk '/image-path.*rw\.[0-9]+\.AirNote/ {p=1} p && /^\/dev\/disk[0-9]+\t/ {print $1; p=0}')

# Remove the temp files themselves.
rm -f "$BUNDLE_DIR"/macos/rw.*.AirNote_*.dmg 2>/dev/null || true
ok "pre-clean done"

# ── Build the Rust sidecar ────────────────────────────────────────────────────
step "Build airnote-backend (release, $TARGET)"
cd "$REPO_ROOT"
# Bust the Cargo fingerprint cache for the binary entry point.
touch crates/backend/src/main.rs
cargo build -p said-backend --release --target "$TARGET"
ok "airnote-backend built"

step "Sync sidecar to Tauri externalBin slot"
mkdir -p "$TAURI_DIR/binaries"
cp "$SIDECAR_SRC" "$SIDECAR_DEST"
chmod +x "$SIDECAR_DEST"
ok "synced to $SIDECAR_DEST"

# ── Tauri build ──────────────────────────────────────────────────────────────
step "Run tauri build (--target $TARGET)"
cd "$DESKTOP_DIR"
[ -d node_modules ] || npm ci
[ -x "$DESKTOP_DIR/node_modules/.bin/create-dmg" ] || npm ci
npm run tauri:build -- --target "$TARGET"
ok "tauri build finished"

# ── Post-verify: Developer ID signing ────────────────────────────────────────
step "Sign with Developer ID"

[ -d "$APP_PATH" ] || fail ".app not found at $APP_PATH"

# Signing identity — set via env var or default to EMIAC's Developer ID.
SIGN_ID="${APPLE_SIGNING_IDENTITY:-Developer ID Application: EMIAC TECHNOLOGIES LIMITED (96ZQGP7L3B)}"
TEAM_ID="${APPLE_TEAM_ID:-96ZQGP7L3B}"

# Do not embed local .env secrets in packaged app builds.
rm -f "$APP_PATH/Contents/MacOS/.env"
ok "no .env embedded; packaged app will use API keys from preferences DB"

# Strip quarantine for local testing.
xattr -cr "$APP_PATH" 2>/dev/null || true

# Sign the embedded sidecar first, then the outer bundle.
EMBEDDED_BACKEND="$APP_PATH/Contents/MacOS/airnote-backend"
[ -x "$EMBEDDED_BACKEND" ] || fail "embedded sidecar not found at $EMBEDDED_BACKEND"

ENTITLEMENTS="$TAURI_DIR/AirNote.entitlements"

codesign --force --options runtime --sign "$SIGN_ID" --entitlements "$ENTITLEMENTS" "$EMBEDDED_BACKEND" 2>&1 | sed 's/^/  /'
ok "sidecar signed"

codesign --force --deep --options runtime --sign "$SIGN_ID" --entitlements "$ENTITLEMENTS" "$APP_PATH" 2>&1 | sed 's/^/  /'
ok "app bundle signed"

# Verify
codesign --verify --deep --strict --verbose=2 "$APP_PATH" 2>&1 | sed 's/^/  /'

ACTUAL_ID=$(codesign -dv "$APP_PATH" 2>&1 | awk -F= '/^Identifier=/ {print $2}')
[ "$ACTUAL_ID" = "$BUNDLE_ID" ] \
  || fail "bundle id mismatch — expected $BUNDLE_ID got $ACTUAL_ID"
ok "outer bundle: Identifier=$ACTUAL_ID, signed with Developer ID"

codesign --verify --strict "$EMBEDDED_BACKEND"
ok "embedded sidecar verified: $(codesign -dv "$EMBEDDED_BACKEND" 2>&1 | awk -F= '/^Identifier=/ {print $2}')"

# ── Build the DMG ────────────────────────────────────────────────────────────
#
# Prefer `create-dmg` for the polished native Finder layout (small drag-to-
# Applications window, sane icon positions). If node dependencies are missing
# or the helper fails, fall back to the plain `hdiutil create -srcfolder` path
# that avoids Finder/osascript automation entirely.
step "Build DMG"

STAGING="$BUNDLE_DIR/dmg-staging"
DMG_OUT="$BUNDLE_DIR/dmg/AirNote_${VERSION}_${ARCH_SHORT}.dmg"
VOLNAME="AirNote"

# Ensure no leftover staging from a prior run.
rm -rf "$STAGING" "$DMG_OUT"
mkdir -p "$STAGING" "$BUNDLE_DIR/dmg"

CREATE_DMG_BIN="$DESKTOP_DIR/node_modules/.bin/create-dmg"
if [ "${AIRNOTE_PLAIN_DMG:-0}" != "1" ] && [ -x "$CREATE_DMG_BIN" ]; then
  DMG_TMP="$BUNDLE_DIR/dmg-create"
  rm -rf "$DMG_TMP"
  mkdir -p "$DMG_TMP"
  if "$CREATE_DMG_BIN" "$APP_PATH" "$DMG_TMP" \
      --overwrite \
      --dmg-title="$VOLNAME" \
      --no-code-sign 2>&1 | sed 's/^/  /'; then
    CREATED_DMG="$(find "$DMG_TMP" -maxdepth 1 -type f -name "*.dmg" -print -quit)"
    if [ -n "$CREATED_DMG" ] && [ -f "$CREATED_DMG" ]; then
      mv "$CREATED_DMG" "$DMG_OUT"
      ok "polished DMG layout created with create-dmg"
    else
      warn "create-dmg completed but produced no DMG — falling back to plain hdiutil"
    fi
  else
    warn "create-dmg failed — falling back to plain hdiutil"
  fi
  rm -rf "$DMG_TMP"
else
  warn "create-dmg unavailable or AIRNOTE_PLAIN_DMG=1 — using plain hdiutil layout"
fi

if [ ! -f "$DMG_OUT" ]; then
  # Stage: .app + /Applications symlink.
  cp -R "$APP_PATH" "$STAGING/AirNote.app"
  ln -s /Applications "$STAGING/Applications"

  hdiutil create \
    -volname "$VOLNAME" \
    -srcfolder "$STAGING" \
    -ov \
    -format UDZO \
    "$DMG_OUT" >/dev/null
fi

rm -rf "$STAGING"

[ -f "$DMG_OUT" ] || fail "hdiutil create did not produce $DMG_OUT"
ok "DMG: $DMG_OUT ($(du -h "$DMG_OUT" | cut -f1 | xargs))"

# Sign the disk image container itself. The .app inside is already signed, but
# Gatekeeper assesses the downloaded DMG first; without this signature spctl
# reports "source=no usable signature" even after notarization succeeds.
codesign --force --sign "$SIGN_ID" "$DMG_OUT" 2>&1 | sed 's/^/  /'
ok "DMG container signed"

# Verify the DMG is openable end-to-end. We attach + detach to make sure
# the volume mounts cleanly and the .app inside still satisfies its DR.
step "Verify DMG mounts cleanly and signature still validates"
ATTACH_OUT=$(hdiutil attach -nobrowse -readonly "$DMG_OUT")
echo "$ATTACH_OUT" | sed 's/^/  /'
DMG_DEV=$(echo "$ATTACH_OUT" | awk '/\/dev\/disk[0-9]+s[0-9]+/ && /\/Volumes\// {print $1; exit}')
DMG_VOL=$(echo "$ATTACH_OUT" | awk '/\/Volumes\// {for (i=3;i<=NF;i++) printf "%s%s", $i, (i<NF?" ":""); print ""; exit}')
codesign --verify --deep --strict --verbose=2 "$DMG_VOL/AirNote.app" 2>&1 | sed 's/^/  /'
hdiutil detach "$DMG_DEV" -force >/dev/null
ok "DMG verified — AirNote.app inside is correctly signed and mounts cleanly"

# ── Notarize the DMG ─────────────────────────────────────────────────────────
# Requires APPLE_ID and APPLE_APP_SPECIFIC_PASSWORD env vars.
# Skip notarization if credentials are not set (local dev builds).
APPLE_ID_EMAIL="${APPLE_ID:-}"
APPLE_ASP="${APPLE_APP_SPECIFIC_PASSWORD:-}"

if [ -n "$APPLE_ID_EMAIL" ] && [ -n "$APPLE_ASP" ]; then
  step "Notarize DMG with Apple"
  NOTARY_TIMEOUT="${NOTARY_TIMEOUT:-25m}"
  xcrun notarytool submit "$DMG_OUT" \
    --apple-id "$APPLE_ID_EMAIL" \
    --team-id "$TEAM_ID" \
    --password "$APPLE_ASP" \
    --timeout "$NOTARY_TIMEOUT" \
    --wait 2>&1 | sed 's/^/  /'
  ok "notarization submitted"

  step "Staple notarization ticket"
  xcrun stapler staple "$DMG_OUT" 2>&1 | sed 's/^/  /'
  ok "stapled"

  step "Verify notarization"
  spctl --assess --type open --context context:primary-signature -v "$DMG_OUT" 2>&1 | sed 's/^/  /'
  ok "DMG is signed + notarized — ready for distribution"
else
  warn "APPLE_ID and APPLE_APP_SPECIFIC_PASSWORD not set — skipping notarization"
  warn "DMG is signed but NOT notarized (users will see 'identified developer' warning)"
fi

# ── Output ───────────────────────────────────────────────────────────────────
step "Done"
echo "  app: $APP_PATH"
echo "  dmg: $DMG_OUT"
echo ""
echo "  Install for testing:"
echo "    open '$DMG_OUT'   # then drag AirNote.app to /Applications"
echo "  or directly:"
echo "    rm -rf /Applications/AirNote.app && cp -R '$APP_PATH' /Applications/AirNote.app && open /Applications/AirNote.app"
echo ""
