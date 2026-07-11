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
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>AirNote</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>2.4.3</string>
  <key>CFBundleVersion</key>
  <string>2.4.3</string>
  <key>CSResourcesFileMapped</key>
  <true/>
  <key>LSRequiresCarbon</key>
  <true/>
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
  <key>NSHighResolutionCapable</key>
  <true/>
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

find_exact_executable_pid() {
  local executable="$1"
  local pid command

  while read -r pid command; do
    case "$command" in
      "$executable"|"$executable "*)
        echo "$pid"
        return 0
        ;;
    esac
  done < <(ps -axo pid=,command=)

  return 1
}

terminate_process() {
  local pid="${1:-}"

  if [ -z "$pid" ] || ! kill -0 "$pid" 2>/dev/null; then
    return 0
  fi

  kill -TERM "$pid" 2>/dev/null || true
  for _ in $(seq 1 40); do
    if ! kill -0 "$pid" 2>/dev/null; then
      return 0
    fi
    sleep 0.05
  done

  kill -KILL "$pid" 2>/dev/null || true
}

launch_app_bundle() {
  local app="$1"
  local executable="$2"
  shift 2

  local open_pid=""
  local launched_pid=""
  local terminal=""
  local status=0

  # `open -a` is the important part here: merely executing the Mach-O leaves it
  # in the terminal launch session, while opening the .app as a document can
  # fail with kLSUnknownErr when multiple AirNote bundles are registered.
  # LaunchServices rejects /dev/stdout as a redirection target, so use the real
  # terminal device when one exists. The app's file logger remains available in
  # non-interactive environments.
  if [ -t 1 ]; then
    terminal="$(tty 2>/dev/null || true)"
  fi

  if [ "$#" -gt 0 ] && [ -n "$terminal" ] && [ "$terminal" != "not a tty" ]; then
    open -W -n --stdout "$terminal" --stderr "$terminal" -a "$app" --args "$@" &
  elif [ "$#" -gt 0 ]; then
    open -W -n -a "$app" --args "$@" &
  elif [ -n "$terminal" ] && [ "$terminal" != "not a tty" ]; then
    open -W -n --stdout "$terminal" --stderr "$terminal" -a "$app" &
  else
    open -W -n -a "$app" &
  fi
  open_pid=$!

  for _ in $(seq 1 100); do
    launched_pid="$(find_exact_executable_pid "$executable" || true)"
    if [ -n "$launched_pid" ]; then
      break
    fi
    if ! kill -0 "$open_pid" 2>/dev/null; then
      break
    fi
    sleep 0.05
  done

  if [ -z "$launched_pid" ]; then
    kill -TERM "$open_pid" 2>/dev/null || true
    wait "$open_pid" 2>/dev/null || true
    echo "tauri-dev-runner: LaunchServices did not start $app" >&2
    return 66
  fi

  stop_launch() {
    local exit_code="$1"
    trap - INT TERM HUP
    terminate_process "$launched_pid"
    kill -TERM "$open_pid" 2>/dev/null || true
    wait "$open_pid" 2>/dev/null || true
    exit "$exit_code"
  }

  # Tauri terminates its runner during rebuilds and on Ctrl-C. Since the app is
  # now owned by launchd rather than this shell, explicitly forward shutdown so
  # a stale AirNote Dev process cannot survive the dev session.
  trap 'stop_launch 130' INT
  trap 'stop_launch 143' TERM HUP

  wait "$open_pid" || status=$?
  trap - INT TERM HUP

  # `open -W` returns when the launched app exits. It may not preserve the
  # application's exact exit status, but a non-zero launcher failure must still
  # reach Tauri so it does not report a healthy dev run.
  return "$status"
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
    local executable="$macos/AirNote"

    # Only target the dev app produced by this worktree. This is safe for the
    # installed AirNote and for other worktrees, while repairing an interrupted
    # previous run before replacing its bundle on disk.
    local stale_pid
    stale_pid="$(find_exact_executable_pid "$executable" || true)"
    if [ -n "$stale_pid" ]; then
      echo "tauri-dev-runner: stopping stale AirNote Dev process $stale_pid" >&2
      terminate_process "$stale_pid"
    fi

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
    launch_app_bundle "$app" "$executable" "$@"
    return $?
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
