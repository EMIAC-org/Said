#!/bin/bash
# Build a local test DMG from the current checkout.
#
# This intentionally does not bump versions, push, or deploy. It does build,
# sign, notarize, staple, and verify the DMG in one command. It disables Tauri
# updater artifacts, so it uses only the Apple credential loaded by
# build-dmg.sh from the macOS Keychain.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="${1:-aarch64-apple-darwin}"

export AIRNOTE_LOCAL_TEST_DMG=1
export AIRNOTE_REQUIRE_NOTARIZATION="${AIRNOTE_REQUIRE_NOTARIZATION:-1}"
export NOTARY_TIMEOUT="${NOTARY_TIMEOUT:-45m}"

"$REPO_ROOT/scripts/build-dmg.sh" "$TARGET"
