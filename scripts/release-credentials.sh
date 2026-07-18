#!/usr/bin/env bash
# Shared, local-only credential loader for signed macOS releases.
#
# Secrets never live in this repository or in a documentation attachment:
#   - Apple notarization password: macOS Keychain
#       account: shivam@emiactech.com, service: airnote-apple-app-password
#   - Tauri updater password: macOS Keychain
#       account: airnote, service: airnote-tauri-updater-private-key-password
#   - Tauri updater private key: ~/.tauri/said-updater.key (mode 0600)
#
# Source this file, then call the loader(s) needed by the invoking command.

airnote_release_error() {
  echo "error: $*" >&2
  return 1
}

airnote_load_notarization_credentials() {
  # Explicit caller-provided credentials or a notarytool keychain profile take
  # precedence. Otherwise use the one production credential stored locally in
  # the login Keychain. This keeps the app-specific password out of shell
  # history, .env files, and release documentation.
  export APPLE_TEAM_ID="${APPLE_TEAM_ID:-96ZQGP7L3B}"
  export APPLE_SIGNING_IDENTITY="${APPLE_SIGNING_IDENTITY:-Developer ID Application: EMIAC TECHNOLOGIES LIMITED (96ZQGP7L3B)}"
  export AIRNOTE_REQUIRE_NOTARIZATION="${AIRNOTE_REQUIRE_NOTARIZATION:-1}"

  if [[ -n "${APPLE_KEYCHAIN_PROFILE:-}" ]]; then
    return 0
  fi

  export APPLE_ID="${APPLE_ID:-shivam@emiactech.com}"
  if [[ -n "${APPLE_APP_SPECIFIC_PASSWORD:-}" || -n "${APPLE_PASSWORD:-}" ]]; then
    return 0
  fi

  if ! command -v security >/dev/null 2>&1; then
    airnote_release_error "macOS Keychain utility 'security' is required for notarization"
    return 1
  fi

  local apple_password
  apple_password="$(
    security find-generic-password \
      -a "$APPLE_ID" \
      -s airnote-apple-app-password \
      -w 2>/dev/null || true
  )"
  if [[ -z "$apple_password" ]]; then
    airnote_release_error \
      "Apple notarization password is missing from Keychain (account '$APPLE_ID', service 'airnote-apple-app-password')"
    return 1
  fi

  export APPLE_APP_SPECIFIC_PASSWORD="$apple_password"
  unset apple_password
}

airnote_load_updater_signing_credentials() {
  # A production Tauri build creates signed updater artifacts. Load the key
  # only for the current process; it is deliberately not read from .env.
  local key_path="${TAURI_SIGNING_PRIVATE_KEY_PATH:-$HOME/.tauri/said-updater.key}"

  if [[ -z "${TAURI_SIGNING_PRIVATE_KEY:-}" ]]; then
    if [[ ! -r "$key_path" ]]; then
      airnote_release_error "Tauri updater private key is missing: $key_path"
      return 1
    fi
    if ! chmod 600 "$key_path"; then
      airnote_release_error "could not secure Tauri updater private key: $key_path"
      return 1
    fi

    local private_key
    private_key="$(<"$key_path")"
    if [[ -z "$private_key" ]]; then
      airnote_release_error "Tauri updater private key is empty: $key_path"
      return 1
    fi
    export TAURI_SIGNING_PRIVATE_KEY="$private_key"
    unset private_key
  fi

  if [[ -n "${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}" ]]; then
    return 0
  fi

  if ! command -v security >/dev/null 2>&1; then
    airnote_release_error "macOS Keychain utility 'security' is required for updater signing"
    return 1
  fi

  local updater_password
  updater_password="$(
    security find-generic-password \
      -a airnote \
      -s airnote-tauri-updater-private-key-password \
      -w 2>/dev/null || true
  )"
  if [[ -z "$updater_password" ]]; then
    airnote_release_error \
      "Tauri updater password is missing from Keychain (account 'airnote', service 'airnote-tauri-updater-private-key-password')"
    return 1
  fi

  export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$updater_password"
  unset updater_password
}
