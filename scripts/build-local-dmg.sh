#!/bin/bash
# Build a local test DMG from the current checkout.
#
# This intentionally does not bump versions, push, deploy, or notarize. It also
# disables Tauri updater artifacts so local builds do not require updater signing
# keys that are only needed for release publishing.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="${1:-aarch64-apple-darwin}"

export AIRNOTE_LOCAL_TEST_DMG=1

# Local test builds should not wait on Apple notarization even if release
# credentials are present in the shell.
unset APPLE_ID
unset APPLE_APP_SPECIFIC_PASSWORD

"$REPO_ROOT/scripts/build-dmg.sh" "$TARGET"
