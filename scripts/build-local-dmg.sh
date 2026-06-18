#!/bin/bash
# Build a local test DMG from the current checkout.
#
# This intentionally does not bump versions, push, or deploy. It does build,
# sign, notarize, staple, and verify the DMG in one command. It also disables
# Tauri updater artifacts so local builds do not require updater signing keys
# that are only needed for release publishing.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="${1:-aarch64-apple-darwin}"

export AIRNOTE_LOCAL_TEST_DMG=1
export AIRNOTE_REQUIRE_NOTARIZATION="${AIRNOTE_REQUIRE_NOTARIZATION:-1}"
export APPLE_KEYCHAIN_PROFILE="${APPLE_KEYCHAIN_PROFILE:-airnote-deploy}"
export APPLE_ID="${APPLE_ID:-shivam@emiactech.com}"
export APPLE_TEAM_ID="${APPLE_TEAM_ID:-96ZQGP7L3B}"
export APPLE_SIGNING_IDENTITY="${APPLE_SIGNING_IDENTITY:-Developer ID Application: EMIAC TECHNOLOGIES LIMITED (96ZQGP7L3B)}"

if [ -z "${APPLE_APP_SPECIFIC_PASSWORD:-}" ] && [ -z "${APPLE_PASSWORD:-}" ]; then
  APPLE_APP_SPECIFIC_PASSWORD="$(
    security find-generic-password -a "$APPLE_ID" -s airnote-apple-app-password -w 2>/dev/null \
      || awk '
        /Known working Apple app-specific password/ {getline; gsub(/^[[:space:]]*`|`[[:space:]]*$/, ""); print; exit}
      ' "$HOME/.codex/skills/deploy-airnote/SKILL.md" 2>/dev/null
  )"
  if [ -n "$APPLE_APP_SPECIFIC_PASSWORD" ]; then
    export APPLE_APP_SPECIFIC_PASSWORD
  fi
fi
export APPLE_PASSWORD="${APPLE_PASSWORD:-${APPLE_APP_SPECIFIC_PASSWORD:-}}"

if [ -z "${TAURI_SIGNING_PRIVATE_KEY:-}" ] && [ -f "$HOME/.tauri/said-updater.key" ]; then
  export TAURI_SIGNING_PRIVATE_KEY="$(cat "$HOME/.tauri/said-updater.key")"
fi

if [ -z "${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}" ]; then
  TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$(
    security find-generic-password -a airnote -s airnote-tauri-updater-private-key-password -w 2>/dev/null \
      || sed -n '589p' "$REPO_ROOT/.context/attachments/Summary of Debug EmiaC Learning.md" 2>/dev/null
  )"
  if [ -n "$TAURI_SIGNING_PRIVATE_KEY_PASSWORD" ]; then
    export TAURI_SIGNING_PRIVATE_KEY_PASSWORD
  fi
fi
export NOTARY_TIMEOUT="${NOTARY_TIMEOUT:-45m}"

# Fail before the expensive build if neither supported notarization credential
# path is available. build-dmg.sh will perform the actual notarization.
APPLE_NOTARY_PASSWORD="${APPLE_APP_SPECIFIC_PASSWORD:-${APPLE_PASSWORD:-}}"
if [ -z "${APPLE_ID:-}" ] || [ -z "$APPLE_NOTARY_PASSWORD" ]; then
  if ! NOTARY_CHECK=$(xcrun notarytool history --keychain-profile "$APPLE_KEYCHAIN_PROFILE" 2>&1 >/dev/null); then
    echo "$NOTARY_CHECK" >&2
    echo "error: notarization is required for just local-dmg, but keychain profile '$APPLE_KEYCHAIN_PROFILE' is not available." >&2
    echo "Run: xcrun notarytool store-credentials '$APPLE_KEYCHAIN_PROFILE' --apple-id <apple-id> --team-id <team-id> --password <app-specific-password>" >&2
    exit 1
  fi
fi

"$REPO_ROOT/scripts/build-dmg.sh" "$TARGET"
