#!/bin/bash
# Build a release DMG of Said.app with a stable ad-hoc signature.
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
APP_PATH="$BUNDLE_DIR/macos/Said.app"
SIDECAR_SRC="$REPO_ROOT/target/$TARGET/release/said-backend"
SIDECAR_DEST="$TAURI_DIR/binaries/said-backend-$TARGET"
BUNDLE_ID="com.emiac.said.desktop"

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
step "Pre-clean: detach stale Said volumes & temp DMGs"

# Detach any volume mounted with the product name (read-only finalized DMGs
# the user mounted, plus any read-write working DMG still attached from a
# prior failed run).
for vol in "/Volumes/Said" "/Volumes/Said 1" "/Volumes/Said 2"; do
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
done < <(hdiutil info | awk '/image-path.*rw\.[0-9]+\.Said/ {p=1} p && /^\/dev\/disk[0-9]+\t/ {print $1; p=0}')

# Remove the temp files themselves.
rm -f "$BUNDLE_DIR"/macos/rw.*.Said_*.dmg 2>/dev/null || true
ok "pre-clean done"

# ── Build the Rust sidecar ────────────────────────────────────────────────────
step "Build said-backend (release, $TARGET)"
cd "$REPO_ROOT"
# Bust the Cargo fingerprint cache for the binary entry point.
touch crates/backend/src/main.rs
cargo build -p said-backend --release --target "$TARGET"
ok "said-backend built"

step "Sync sidecar to Tauri externalBin slot"
mkdir -p "$TAURI_DIR/binaries"
cp "$SIDECAR_SRC" "$SIDECAR_DEST"
chmod +x "$SIDECAR_DEST"
ok "synced to $SIDECAR_DEST"

# ── Tauri build ──────────────────────────────────────────────────────────────
step "Run tauri build (--target $TARGET)"
cd "$DESKTOP_DIR"
[ -d node_modules ] || npm ci
npm run tauri:build -- --target "$TARGET"
ok "tauri build finished"

# ── Post-verify: ensure deep ad-hoc signature ────────────────────────────────
step "Re-sign deep (ad-hoc) and verify"

[ -d "$APP_PATH" ] || fail ".app not found at $APP_PATH"

# Copy local-only runtime secrets into this build when a root .env exists.
# .env is gitignored; the copy lands under target/ and is included only in the
# locally produced app/DMG.
if [ -f "$REPO_ROOT/.env" ]; then
  cp "$REPO_ROOT/.env" "$APP_PATH/Contents/MacOS/.env"
  chmod 600 "$APP_PATH/Contents/MacOS/.env"
  ok "embedded local .env into app bundle"
else
  warn "no root .env found; packaged app will rely on external runtime env"
fi

# Strip quarantine so future user-side `xattr -dr com.apple.quarantine` is
# unnecessary for local testing.
xattr -cr "$APP_PATH" 2>/dev/null || true

# Re-sign deep — Tauri's `signingIdentity: "-"` does sign the outer bundle,
# but historically does not reliably deep-sign embedded sidecar binaries
# (tauri-apps/tauri#11992). Doing it ourselves guarantees the sidecar carries
# a matching ad-hoc signature so TCC sees a single coherent bundle.
codesign --force --deep --sign - "$APP_PATH" 2>&1 | sed 's/^/  /'

# Verify
codesign --verify --deep --strict --verbose=2 "$APP_PATH" 2>&1 | sed 's/^/  /'

ACTUAL_ID=$(codesign -dv "$APP_PATH" 2>&1 | awk -F= '/^Identifier=/ {print $2}')
[ "$ACTUAL_ID" = "$BUNDLE_ID" ] \
  || fail "bundle id mismatch — expected $BUNDLE_ID got $ACTUAL_ID"
ok "outer bundle: Identifier=$ACTUAL_ID, signature=adhoc"

# Tauri strips the target triple when injecting externalBin into the bundle —
# the file inside Contents/MacOS is `said-backend`, not the triple-suffixed
# source name.
EMBEDDED_BACKEND="$APP_PATH/Contents/MacOS/said-backend"
[ -x "$EMBEDDED_BACKEND" ] || fail "embedded sidecar not found at $EMBEDDED_BACKEND"
codesign --verify --strict "$EMBEDDED_BACKEND"
ok "embedded sidecar signed: $(codesign -dv "$EMBEDDED_BACKEND" 2>&1 | awk -F= '/^Identifier=/ {print $2}')"

# ── Build the DMG ourselves with plain hdiutil ───────────────────────────────
#
# We don't use Tauri's DMG target because its bundle_dmg.sh shells out to
# osascript to drive Finder for cosmetic icon layout, which fails
# non-deterministically on recent macOS (tauri-apps/tauri#3055,
# community#163491). A plain `hdiutil create -srcfolder` with an /Applications
# symlink is what users actually need: drag-to-install, signature intact.
step "Build DMG with hdiutil"

STAGING="$BUNDLE_DIR/dmg-staging"
DMG_OUT="$BUNDLE_DIR/dmg/Said_${VERSION}_${ARCH_SHORT}.dmg"
VOLNAME="Said"

# Ensure no leftover staging from a prior run.
rm -rf "$STAGING" "$DMG_OUT"
mkdir -p "$STAGING" "$BUNDLE_DIR/dmg"

# Stage: .app + /Applications symlink + a .DS_Store that sets the window
# size and icon positions so Finder opens a proper drag-to-install layout.
cp -R "$APP_PATH" "$STAGING/Said.app"
ln -s /Applications "$STAGING/Applications"

# ── Inject window layout via a writable interim DMG ──────────────────────
# We mount a temporary read-write image, use osascript to position the two
# icons side-by-side (app on the left, Applications folder on the right),
# then convert to the final read-only UDZO for distribution.
RW_DMG="$BUNDLE_DIR/dmg/Said_rw.dmg"
rm -f "$RW_DMG"

hdiutil create \
  -volname "$VOLNAME" \
  -srcfolder "$STAGING" \
  -ov \
  -format UDRW \
  -fs HFS+ \
  "$RW_DMG" >/dev/null

RW_VOL=$(hdiutil attach "$RW_DMG" -readwrite -nobrowse | awk '/\/Volumes\// {for(i=3;i<=NF;i++) printf "%s%s",$i,(i<NF?" ":""); print ""; exit}')
ok "mounted rw volume at $RW_VOL"

# Set icon positions: Said.app at (150,180), Applications at (410,180).
# Window: 560×340, icon size 128px, no toolbar, no sidebar.
osascript <<APPLESCRIPT >/dev/null 2>&1 || true
tell application "Finder"
  tell disk "$VOLNAME"
    open
    set current view of container window to icon view
    set toolbar visible of container window to false
    set statusbar visible of container window to false
    set the bounds of container window to {400, 200, 960, 540}
    set theViewOptions to the icon view options of container window
    set arrangement of theViewOptions to not arranged
    set icon size of theViewOptions to 128
    set position of item "Said.app"    of container window to {150, 180}
    set position of item "Applications" of container window to {410, 180}
    close
    open
    update without registering applications
    delay 2
    close
  end tell
end tell
APPLESCRIPT

# Flush Finder's DS_Store writes to disk before we detach.
sync
hdiutil detach "$RW_VOL" -force >/dev/null

# Convert the laid-out rw image → final compressed read-only DMG.
hdiutil convert "$RW_DMG" -format UDZO -o "$DMG_OUT" >/dev/null
rm -f "$RW_DMG"
rm -rf "$STAGING"

[ -f "$DMG_OUT" ] || fail "hdiutil create did not produce $DMG_OUT"
ok "DMG: $DMG_OUT ($(du -h "$DMG_OUT" | cut -f1 | xargs))"

# Verify the DMG is openable end-to-end. We attach + detach to make sure
# the volume mounts cleanly and the .app inside still satisfies its DR.
step "Verify DMG mounts cleanly and signature still validates"
ATTACH_OUT=$(hdiutil attach -nobrowse -readonly "$DMG_OUT")
echo "$ATTACH_OUT" | sed 's/^/  /'
DMG_DEV=$(echo "$ATTACH_OUT" | awk '/\/dev\/disk[0-9]+s[0-9]+/ && /\/Volumes\// {print $1; exit}')
DMG_VOL=$(echo "$ATTACH_OUT" | awk '/\/Volumes\// {for (i=3;i<=NF;i++) printf "%s%s", $i, (i<NF?" ":""); print ""; exit}')
codesign --verify --deep --strict --verbose=2 "$DMG_VOL/Said.app" 2>&1 | sed 's/^/  /'
hdiutil detach "$DMG_DEV" -force >/dev/null
ok "DMG verified — Said.app inside is correctly signed and mounts cleanly"

# ── Output ───────────────────────────────────────────────────────────────────
step "Done"
echo "  app: $APP_PATH"
echo "  dmg: $DMG_OUT"
echo ""
echo "  Install for testing:"
echo "    open '$DMG_OUT'   # then drag Said.app to /Applications"
echo "  or directly:"
echo "    rm -rf /Applications/Said.app && cp -R '$APP_PATH' /Applications/Said.app && open /Applications/Said.app"
echo ""
