#!/usr/bin/env bash
# Publish a mock update: sign a bundle with the TEST key and write a latest.json
# manifest that the app's updater would consume.
#
# Usage:
#   ./publish.sh <version> [path-to-bundle]
#
#   <version>        e.g. 99.0.0  (use a high version to force "update available")
#   [path-to-bundle] the .app.tar.gz (macOS) or -setup.exe (windows) to serve.
#                    If omitted, a dummy placeholder bundle is created so you can
#                    exercise the manifest/sign/serve/validate path without a build.
#
# Output: tools/update-harness/mock/{latest.json, <bundle>, <bundle>.sig}
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
KEY="$HERE/keys/test.key"
MOCK="$HERE/mock"
PORT="${HARNESS_PORT:-3007}"

VERSION="${1:?usage: publish.sh <version> [bundle]}"
BUNDLE_SRC="${2:-}"

[[ -f "$KEY" ]] || { echo "no test key — run ./gen-keys.sh first"; exit 1; }

# Detect this machine's Tauri platform key (OS-ARCH).
case "$(uname -s)" in Darwin) OS=darwin ;; Linux) OS=linux ;; *) OS=windows ;; esac
case "$(uname -m)" in arm64|aarch64) ARCH=aarch64 ;; *) ARCH=x86_64 ;; esac
PLATFORM="${OS}-${ARCH}"

mkdir -p "$MOCK"

# If no real bundle given, fabricate a placeholder so the sign/serve/validate
# path is fully exercised. (Phase 2 swaps in a real signed bundle for install.)
if [[ -z "$BUNDLE_SRC" ]]; then
  BUNDLE="AirNote_${VERSION}_${ARCH}.app.tar.gz"
  printf 'mock airnote update bundle v%s\n' "$VERSION" > "$MOCK/_payload.txt"
  tar czf "$MOCK/$BUNDLE" -C "$MOCK" _payload.txt
  rm -f "$MOCK/_payload.txt"
  echo "✓ fabricated placeholder bundle: $BUNDLE"
else
  BUNDLE="$(basename "$BUNDLE_SRC")"
  cp "$BUNDLE_SRC" "$MOCK/$BUNDLE"
  echo "✓ copied bundle: $BUNDLE"
fi

# Sign the bundle with the TEST key → produces <bundle>.sig next to it.
( cd "$HERE/../../desktop"
  npx tauri signer sign -k "$(cat "$KEY")" -p "" "$MOCK/$BUNDLE" >/dev/null )
SIG="$(cat "$MOCK/$BUNDLE.sig")"
echo "✓ signed → $BUNDLE.sig"

# Write the manifest exactly as the Tauri updater expects.
PUB_DATE="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
cat > "$MOCK/latest.json" <<JSON
{
  "version": "${VERSION}",
  "notes": "Harness test build ${VERSION}",
  "pub_date": "${PUB_DATE}",
  "platforms": {
    "${PLATFORM}": {
      "signature": "${SIG}",
      "url": "http://localhost:${PORT}/${BUNDLE}"
    }
  }
}
JSON

echo "✓ wrote manifest: $MOCK/latest.json  (platform=${PLATFORM}, version=${VERSION})"
echo
echo "next: ./serve.sh   then in another shell   ./smoke.sh"
