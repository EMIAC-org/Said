#!/bin/bash
# Cargo shim for `tauri dev --runner`.
#
# Tauri dev normally launches target/debug/said-desktop as a raw Mach-O. macOS
# TCC then keys Accessibility/Input Monitoring to an unstable ad-hoc signature
# identifier like `said_desktop-...`, so grants can fail to stick after rebuilds.
# Re-sign the just-built dev binary with AirNote's real bundle id before launch.

set -eo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"

sign_and_exec() {
  local bin="$1"
  shift

  if [ -f "$bin" ]; then
    local identity="${AIRNOTE_DEV_CODESIGN_IDENTITY:-Developer ID Application: EMIAC TECHNOLOGIES LIMITED (96ZQGP7L3B)}"
    if ! security find-identity -v -p codesigning 2>/dev/null | grep -Fq "$identity"; then
      identity="-"
    fi

    codesign --force --sign "$identity" --identifier com.emiac.airnote.desktop "$bin" >/dev/null 2>&1 || {
      echo "tauri-dev-runner: codesign failed for $bin" >&2
      exit 65
    }
  fi

  export __CFBundleIdentifier=com.emiac.airnote.desktop
  exec "$bin" "$@"
}

if [ "$#" -lt 1 ]; then
  echo "tauri-dev-runner: missing command" >&2
  exit 64
fi

if [ "$1" = "run" ]; then
  shift

  build_args=()
  app_args=()
  seen_separator=0
  for arg in "$@"; do
    if [ "$arg" = "--" ] && [ "$seen_separator" -eq 0 ]; then
      seen_separator=1
      continue
    fi
    if [ "$seen_separator" -eq 0 ]; then
      build_args+=("$arg")
    else
      app_args+=("$arg")
    fi
  done

  cargo build "${build_args[@]}"
  sign_and_exec "$repo_root/target/debug/said-desktop" "${app_args[@]}"
fi

# Fallback for direct Cargo-runner style use: first argument is the executable.
bin="$1"
shift
sign_and_exec "$bin" "$@"
