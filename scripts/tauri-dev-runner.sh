#!/bin/bash
# Cargo shim for `tauri dev --runner`.
#
# Tauri dev normally launches target/debug/said-desktop as a raw Mach-O. macOS
# then gives it terminal-style activation semantics, which can differ from the
# real packaged .app and eat clicks on cold/onboarding launches. Run the freshly
# built debug binary from a tiny .app wrapper so dev behaves like the artifact.

set -eo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"

codesign_path() {
  local path="$1"
  local identity="${AIRNOTE_DEV_CODESIGN_IDENTITY:-Developer ID Application: EMIAC TECHNOLOGIES LIMITED (96ZQGP7L3B)}"
  if ! security find-identity -v -p codesigning 2>/dev/null | grep -Fq "$identity"; then
    identity="-"
  fi

  codesign --force --sign "$identity" --identifier com.emiac.airnote.desktop "$path" >/dev/null 2>&1 || {
    echo "tauri-dev-runner: codesign failed for $path" >&2
    exit 65
  }
}

write_dev_info_plist() {
  local plist="$1"
  cat > "$plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleDisplayName</key>
  <string>AirNote Dev</string>
  <key>CFBundleExecutable</key>
  <string>AirNote</string>
  <key>CFBundleIdentifier</key>
  <string>com.emiac.airnote.desktop</string>
  <key>CFBundleName</key>
  <string>AirNote</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>2.4.3</string>
  <key>CFBundleVersion</key>
  <string>2.4.3</string>
  <key>LSMinimumSystemVersion</key>
  <string>13.0</string>
  <key>NSAccessibilityUsageDescription</key>
  <string>AirNote needs Accessibility access to paste polished text directly into other apps.</string>
  <key>NSAppleEventsUsageDescription</key>
  <string>AirNote uses Apple Events to paste text into other apps.</string>
  <key>NSInputMonitoringUsageDescription</key>
  <string>AirNote needs Input Monitoring to detect your recording hotkey and global shortcuts.</string>
  <key>NSMicrophoneUsageDescription</key>
  <string>AirNote records your voice to transcribe and polish your text.</string>
  <key>NSScreenCaptureUsageDescription</key>
  <string>AirNote captures system audio during meetings so it can transcribe what other participants say.</string>
  <key>CFBundleURLTypes</key>
  <array>
    <dict>
      <key>CFBundleURLName</key>
      <string>com.emiac.airnote.desktop</string>
      <key>CFBundleURLSchemes</key>
      <array>
        <string>airnote</string>
      </array>
    </dict>
  </array>
</dict>
</plist>
PLIST
}

copy_if_exists() {
  local src="$1"
  local dst="$2"
  if [ -x "$src" ] || [ -f "$src" ]; then
    cp "$src" "$dst"
    chmod +x "$dst" 2>/dev/null || true
  fi
}

sign_and_exec() {
  local bin="$1"
  shift

  if [ -f "$bin" ]; then
    local host_target
    host_target="$(rustc -vV | awk '/^host:/ {print $2}')"
    local app="$repo_root/target/debug/AirNote Dev.app"
    local contents="$app/Contents"
    local macos="$contents/MacOS"
    local resources="$contents/Resources"

    rm -rf "$app"
    mkdir -p "$macos" "$resources"
    write_dev_info_plist "$contents/Info.plist"
    printf 'APPL????' > "$contents/PkgInfo"

    cp "$bin" "$macos/AirNote"
    chmod +x "$macos/AirNote"

    copy_if_exists "$repo_root/target/debug/airnote-backend" "$macos/airnote-backend"
    copy_if_exists "$repo_root/desktop/src-tauri/binaries/airnote-backend-$host_target" "$macos/airnote-backend"
    copy_if_exists "$repo_root/target/$host_target/release/whisper-cli" "$macos/whisper-cli"
    copy_if_exists "$repo_root/desktop/src-tauri/binaries/whisper-cli-$host_target" "$macos/whisper-cli"

    if [ -f "$repo_root/desktop/src-tauri/resources/models/ggml-silero-v5.1.2.bin" ]; then
      mkdir -p "$resources/models"
      cp "$repo_root/desktop/src-tauri/resources/models/ggml-silero-v5.1.2.bin" "$resources/models/"
    fi

    codesign_path "$macos/AirNote"
    [ -f "$macos/airnote-backend" ] && codesign_path "$macos/airnote-backend"
    [ -f "$macos/whisper-cli" ] && codesign_path "$macos/whisper-cli"
    codesign_path "$app"

    export __CFBundleIdentifier=com.emiac.airnote.desktop
    exec "$macos/AirNote" "$@"
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
