#!/usr/bin/env bash
# Generate a TEST minisign keypair for the update harness.
#
# This is a throwaway key used ONLY for local update testing. NEVER put the
# production TAURI_SIGNING_PRIVATE_KEY here. The test public key is what Phase 2
# bakes into the app (via the dev endpoint override) so the app will accept
# bundles signed by this test key.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
KEYDIR="$HERE/keys"
mkdir -p "$KEYDIR"

if [[ -f "$KEYDIR/test.key" ]]; then
  echo "✓ test key already exists: $KEYDIR/test.key"
else
  ( cd "$HERE/../../desktop"
    npx tauri signer generate --ci -p "" -w "$KEYDIR/test.key" )
  echo "✓ generated $KEYDIR/test.key (+ .pub)"
fi

echo
echo "── TEST PUBLIC KEY (for the Phase 2 app override) ──"
cat "$KEYDIR/test.key.pub"
