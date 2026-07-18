#!/usr/bin/env bash
# Upload a locally built AirNote macOS release to the static updater host.
#
# This script intentionally does not build or notarize the app. Run
# scripts/build-dmg.sh first on a Mac with the Developer ID certificate and
# Apple notarization credentials available. This deploy step only packages the
# Tauri updater archive, signs that updater archive, writes the Darwin updater
# manifest, uploads everything to the VM, and prunes old versions. It is
# intentionally Mac-only and does not overwrite the Windows updater manifest.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

PRODUCT_NAME="${PRODUCT_NAME:-AirNote}"
TARGET="${1:-aarch64-apple-darwin}"
case "$TARGET" in
  aarch64-apple-darwin) ARCH_SHORT="aarch64"; PLATFORM_KEY="darwin-aarch64" ;;
  *) echo "unsupported target: $TARGET (currently only aarch64-apple-darwin is deployable)" >&2; exit 1 ;;
esac

VERSION=$(awk '
  /^\[workspace\.package\]/ { in_section = 1; next }
  /^\[/                     { in_section = 0 }
  in_section && /^[[:space:]]*version[[:space:]]*=/ {
    gsub(/.*=[[:space:]]*"/, "")
    gsub(/".*/, "")
    print
    exit
  }
' Cargo.toml)
[ -n "$VERSION" ] || { echo "could not parse workspace version" >&2; exit 1; }

PUBLIC_BASE_URL="${PUBLIC_BASE_URL:-https://airnote.emiactech.com}"
REMOTE="${REMOTE:-root@103.180.163.41}"
REMOTE_RELEASE_ROOT="${REMOTE_RELEASE_ROOT:-/opt/airnote-control-plane/releases}"
KEEP_RELEASES="${KEEP_RELEASES:-3}"

SSH_CMD=(ssh -o StrictHostKeyChecking=accept-new)
SCP_CMD=(scp -o StrictHostKeyChecking=accept-new)
if [ -n "${SSHPASS:-}" ] && command -v sshpass >/dev/null 2>&1; then
  SSH_CMD=(sshpass -e ssh -o StrictHostKeyChecking=accept-new)
  SCP_CMD=(sshpass -e scp -o StrictHostKeyChecking=accept-new)
fi

APP_DIR="target/$TARGET/release/bundle/macos/$PRODUCT_NAME.app"
DMG_PATH="target/$TARGET/release/bundle/dmg/${PRODUCT_NAME}_${VERSION}_${ARCH_SHORT}.dmg"
ARTIFACT_DIR=".context/release-artifacts/$VERSION"
UPDATE_TAR="${PRODUCT_NAME}_${VERSION}_${ARCH_SHORT}.app.tar.gz"
UPDATE_TAR_PATH="$ARTIFACT_DIR/$UPDATE_TAR"
UPDATE_SIG_PATH="$UPDATE_TAR_PATH.sig"
DARWIN_MANIFEST_PATH="$ARTIFACT_DIR/darwin-latest.json"
LEGACY_MANIFEST_PATH="$ARTIFACT_DIR/latest.json"

require_file() {
  [ -f "$1" ] || { echo "missing required file: $1" >&2; exit 1; }
}

require_dir() {
  [ -d "$1" ] || { echo "missing required directory: $1" >&2; exit 1; }
}

require_dir "$APP_DIR"
require_file "$DMG_PATH"

if [ "${SKIP_GATEKEEPER_VERIFY:-0}" != "1" ]; then
  echo "Verifying notarized DMG with Gatekeeper..."
  spctl --assess --type open --context context:primary-signature -v "$DMG_PATH"
fi

# Use exactly the same secure local updater-key source as `just dmg`; no
# release command needs a manual secret export or a plaintext documentation
# fallback.
source "$REPO_ROOT/scripts/release-credentials.sh"
airnote_load_updater_signing_credentials || exit 1

rm -rf "$ARTIFACT_DIR"
mkdir -p "$ARTIFACT_DIR"

echo "Creating updater archive: $UPDATE_TAR"
(
  cd "target/$TARGET/release/bundle/macos"
  # macOS bsdtar otherwise writes AppleDouble `._*` sidecar entries for
  # extended attributes. Tauri's updater tries to unpack those as real app
  # paths and fails with errors like `failed to unpack ._AirNote.app`.
  COPYFILE_DISABLE=1 tar --no-xattrs -czf "$REPO_ROOT/$UPDATE_TAR_PATH" "$PRODUCT_NAME.app"
)

python3 - "$UPDATE_TAR_PATH" <<'PY'
import os
import sys
import tarfile

archive_path = sys.argv[1]
with tarfile.open(archive_path, "r:gz") as archive:
    appledouble = [
        member.name
        for member in archive.getmembers()
        if os.path.basename(member.name).startswith("._")
    ]

if appledouble:
    print("updater archive contains macOS AppleDouble sidecar files:", file=sys.stderr)
    for name in appledouble[:20]:
        print(f"  {name}", file=sys.stderr)
    raise SystemExit(1)
PY

echo "Signing updater archive..."
(
  cd desktop
  npx tauri signer sign \
    "$REPO_ROOT/$UPDATE_TAR_PATH" \
    --private-key "$TAURI_SIGNING_PRIVATE_KEY" \
    --password "$TAURI_SIGNING_PRIVATE_KEY_PASSWORD"
)

require_file "$UPDATE_SIG_PATH"
cp "$DMG_PATH" "$ARTIFACT_DIR/"

PUB_DATE="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
SIGNATURE="$(cat "$UPDATE_SIG_PATH")"
cat > "$DARWIN_MANIFEST_PATH" <<JSON
{
  "version": "$VERSION",
  "notes": "AirNote $VERSION",
  "pub_date": "$PUB_DATE",
  "platforms": {
    "$PLATFORM_KEY": {
      "signature": "$SIGNATURE",
      "url": "$PUBLIC_BASE_URL/releases/$VERSION/$UPDATE_TAR"
    }
  }
}
JSON

if [ "${UPDATE_LEGACY_COMBINED_MANIFEST:-0}" = "1" ]; then
  cp "$DARWIN_MANIFEST_PATH" "$LEGACY_MANIFEST_PATH"
fi

(
  cd "$ARTIFACT_DIR"
  shasum -a 256 * | sort -k 2 > SHA256SUMS
)

echo "Uploading macOS release $VERSION to $REMOTE:$REMOTE_RELEASE_ROOT"
"${SSH_CMD[@]}" "$REMOTE" "mkdir -p '$REMOTE_RELEASE_ROOT/releases/$VERSION' '$REMOTE_RELEASE_ROOT/updates/darwin'"
"${SCP_CMD[@]}" "$ARTIFACT_DIR/$UPDATE_TAR" "$ARTIFACT_DIR/$UPDATE_TAR.sig" "$ARTIFACT_DIR/$(basename "$DMG_PATH")" "$ARTIFACT_DIR/SHA256SUMS" \
  "$REMOTE:$REMOTE_RELEASE_ROOT/releases/$VERSION/"
"${SCP_CMD[@]}" "$DARWIN_MANIFEST_PATH" "$REMOTE:$REMOTE_RELEASE_ROOT/updates/darwin/latest.json"
"${SCP_CMD[@]}" "$DARWIN_MANIFEST_PATH" "$REMOTE:$REMOTE_RELEASE_ROOT/releases/$VERSION/darwin-latest.json"

if [ "${UPDATE_LEGACY_COMBINED_MANIFEST:-0}" = "1" ]; then
  "${SCP_CMD[@]}" "$LEGACY_MANIFEST_PATH" "$REMOTE:$REMOTE_RELEASE_ROOT/updates/latest.json"
  "${SCP_CMD[@]}" "$LEGACY_MANIFEST_PATH" "$REMOTE:$REMOTE_RELEASE_ROOT/releases/$VERSION/latest.json"
fi

"${SSH_CMD[@]}" "$REMOTE" "REMOTE_RELEASE_ROOT='$REMOTE_RELEASE_ROOT' KEEP_RELEASES='$KEEP_RELEASES' bash -s" <<'REMOTE_SCRIPT'
set -euo pipefail
cd "$REMOTE_RELEASE_ROOT/releases"
mapfile -t versions < <(find . -mindepth 1 -maxdepth 1 -type d -print | sed 's#^\./##' | sort -V)
mapfile -t preserve_versions < <(python3 - <<'PY'
import json
import os
import re
from pathlib import Path

root = Path(os.environ["REMOTE_RELEASE_ROOT"])
versions = set()
for path in [
    root / "updates" / "darwin" / "latest.json",
    root / "updates" / "windows" / "latest.json",
    root / "updates" / "latest.json",
]:
    if not path.exists():
        continue
    try:
        data = json.loads(path.read_text())
    except Exception:
        continue
    for entry in (data.get("platforms") or {}).values():
        url = entry.get("url") if isinstance(entry, dict) else ""
        match = re.search(r"/releases/([^/]+)/", url or "")
        if match:
            versions.add(match.group(1))
for version in sorted(versions):
    print(version)
PY
)

is_preserved_version() {
  local candidate="$1"
  local preserved
  for preserved in "${preserve_versions[@]}"; do
    [ "$candidate" = "$preserved" ] && return 0
  done
  return 1
}

current_count=${#versions[@]}
if [ "$current_count" -gt "$KEEP_RELEASES" ]; then
  for old in "${versions[@]}"; do
    [ "$current_count" -le "$KEEP_RELEASES" ] && break
    if is_preserved_version "$old"; then
      echo "Preserving release still referenced by updater manifest: $old"
      continue
    fi
    echo "Pruning old release: $old"
    rm -rf -- "$old"
    current_count=$((current_count - 1))
  done
fi
REMOTE_SCRIPT

echo "Release uploaded:"
echo "  manifest: $PUBLIC_BASE_URL/updates/darwin/latest.json"
echo "  manual DMG: $PUBLIC_BASE_URL/releases/$VERSION/$(basename "$DMG_PATH")"
echo "  updater: $PUBLIC_BASE_URL/releases/$VERSION/$UPDATE_TAR"
if [ "${UPDATE_LEGACY_COMBINED_MANIFEST:-0}" = "1" ]; then
  echo "  legacy combined manifest also updated: $PUBLIC_BASE_URL/updates/latest.json"
else
  echo "  windows manifest left untouched"
fi
