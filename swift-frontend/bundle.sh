#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BUILD_DIR="$SCRIPT_DIR/.build/release"
APP_DIR="${SAID_APP_DIR:-$REPO_ROOT/target/Said.app}"
CONTENTS="$APP_DIR/Contents"
MACOS="$CONTENTS/MacOS"
FRAMEWORKS="$CONTENTS/Frameworks"
RESOURCES="$CONTENTS/Resources"
BUNDLE_ID="com.emiac.said"
CODESIGN_IDENTITY="${CODESIGN_IDENTITY:--}"
VERSION="${SAID_VERSION:-}"
BUILD_NUMBER="${SAID_BUILD_NUMBER:-}"
SPARKLE_FEED_URL="${SPARKLE_FEED_URL:-https://emiac-org.github.io/Said/appcast.xml}"
SPARKLE_PUBLIC_ED_KEY="${SPARKLE_PUBLIC_ED_KEY:-gJ3KhuNYyqsnBpU+6thWoU4krPnsdXP5lCCbdz+1Cak=}"
CODESIGN_REQUIRE_STABLE="${CODESIGN_REQUIRE_STABLE:-0}"
CODESIGN_RUNTIME="${CODESIGN_RUNTIME:-1}"
CODESIGN_TIMESTAMP="${CODESIGN_TIMESTAMP:-0}"
SIGN_ARGS=(--force --sign "$CODESIGN_IDENTITY")

if [ "$CODESIGN_IDENTITY" = "-" ] && [ "$CODESIGN_REQUIRE_STABLE" = "1" ]; then
    echo "stable code signing is required for release builds; set CODESIGN_IDENTITY"
    exit 1
fi

if [ "$CODESIGN_IDENTITY" != "-" ]; then
    if [ "$CODESIGN_RUNTIME" = "1" ]; then
        SIGN_ARGS+=(--options runtime)
    fi
    if [ "$CODESIGN_TIMESTAMP" = "1" ]; then
        SIGN_ARGS+=(--timestamp)
    fi
fi

if [ -z "$VERSION" ]; then
    VERSION=$(awk '
      /^\[workspace\.package\]/ { in_section = 1; next }
      /^\[/                     { in_section = 0 }
      in_section && /^[[:space:]]*version[[:space:]]*=/ {
        gsub(/.*=[[:space:]]*"/, "")
        gsub(/".*/, "")
        print
        exit
      }
    ' "$REPO_ROOT/Cargo.toml")
fi

if [ -z "$BUILD_NUMBER" ]; then
    BUILD_NUMBER="$(date +%Y%m%d%H%M)"
fi

[ -n "$VERSION" ] || { echo "could not determine Said version"; exit 1; }

echo "▶ Building release..."
cd "$SCRIPT_DIR"
swift build --configuration release

echo "▶ Creating app bundle..."
rm -rf "$APP_DIR"
mkdir -p "$MACOS" "$FRAMEWORKS" "$RESOURCES"

# Copy binary
cp "$BUILD_DIR/Said" "$MACOS/Said"
chmod +x "$MACOS/Said"

# Copy SwiftPM resources
if [ -d "$BUILD_DIR/Said_Said.bundle" ]; then
    ditto "$BUILD_DIR/Said_Said.bundle" "$RESOURCES/Said_Said.bundle"
    echo "  ✓ SwiftPM resources bundled"
fi

# Copy Sparkle framework from SwiftPM artifacts into the app bundle.
SPARKLE_FRAMEWORK="$(find "$SCRIPT_DIR/.build" -path '*/Sparkle.framework' -type d -print -quit 2>/dev/null || true)"
if [ -n "$SPARKLE_FRAMEWORK" ]; then
    ditto "$SPARKLE_FRAMEWORK" "$FRAMEWORKS/Sparkle.framework"
    install_name_tool -add_rpath "@executable_path/../Frameworks" "$MACOS/Said" 2>/dev/null || true
    echo "  ✓ Sparkle.framework bundled"
else
    echo "  ⚠ Sparkle.framework not found; app may not launch outside swift build"
fi

# Copy said-backend sidecar
SIDECAR_SRC="${SAID_BACKEND_PATH:-$REPO_ROOT/target/release/said-backend}"
if [ -f "$SIDECAR_SRC" ]; then
    cp "$SIDECAR_SRC" "$MACOS/said-backend"
    chmod +x "$MACOS/said-backend"
    echo "  ✓ said-backend bundled"
fi

# Copy .env if exists
if [ -f "$REPO_ROOT/.env" ]; then
    cp "$REPO_ROOT/.env" "$MACOS/.env"
    echo "  ✓ .env bundled"
fi

# Info.plist
cat > "$CONTENTS/Info.plist" << 'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key>
    <string>com.emiac.said</string>
    <key>CFBundleName</key>
    <string>Said</string>
    <key>CFBundleDisplayName</key>
    <string>Said</string>
    <key>CFBundleExecutable</key>
    <string>Said</string>
    <key>CFBundleVersion</key>
    <string>__SAID_BUILD_NUMBER__</string>
    <key>CFBundleShortVersionString</key>
    <string>__SAID_VERSION__</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>LSMinimumSystemVersion</key>
    <string>15.0</string>
    <key>LSUIElement</key>
    <true/>
    <key>NSMicrophoneUsageDescription</key>
    <string>Said records your voice for dictation.</string>
    <key>NSInputMonitoringUsageDescription</key>
    <string>Said needs Input Monitoring to detect your recording hotkey.</string>
    <key>NSAppleEventsUsageDescription</key>
    <string>Said uses Accessibility to paste polished text into your apps.</string>
    <key>NSAppTransportSecurity</key>
    <dict>
        <key>NSAllowsLocalNetworking</key>
        <true/>
    </dict>
    <key>SUEnableDownloaderService</key>
    <true/>
    <key>SUEnableInstallerLauncherService</key>
    <true/>
    <key>SUFeedURL</key>
    <string>__SPARKLE_FEED_URL__</string>
    <key>SUPublicEDKey</key>
    <string>__SPARKLE_PUBLIC_ED_KEY__</string>
</dict>
</plist>
PLIST

export VERSION BUILD_NUMBER SPARKLE_FEED_URL SPARKLE_PUBLIC_ED_KEY
perl -0pi -e '
  s/__SAID_VERSION__/$ENV{VERSION}/g;
  s/__SAID_BUILD_NUMBER__/$ENV{BUILD_NUMBER}/g;
  s/__SPARKLE_FEED_URL__/$ENV{SPARKLE_FEED_URL}/g;
  s/__SPARKLE_PUBLIC_ED_KEY__/$ENV{SPARKLE_PUBLIC_ED_KEY}/g;
' "$CONTENTS/Info.plist"

if [ -z "$SPARKLE_PUBLIC_ED_KEY" ]; then
    echo "  ⚠ SPARKLE_PUBLIC_ED_KEY is empty; Sparkle updater is disabled for this build"
fi

echo "▶ Signing..."
if [ "$CODESIGN_IDENTITY" = "-" ]; then
    echo "  ⚠ ad-hoc signing; macOS permissions may reset after updates"
else
    echo "  ✓ signing with identity: $CODESIGN_IDENTITY"
fi

if [ -f "$MACOS/said-backend" ]; then
    codesign "${SIGN_ARGS[@]}" "$MACOS/said-backend" 2>&1 | sed 's/^/  /'
fi
if [ -d "$FRAMEWORKS/Sparkle.framework" ]; then
    codesign "${SIGN_ARGS[@]}" "$FRAMEWORKS/Sparkle.framework" 2>&1 | sed 's/^/  /'
fi
codesign "${SIGN_ARGS[@]}" --deep "$APP_DIR" 2>&1 | sed 's/^/  /'
codesign --verify --deep --strict "$APP_DIR" 2>&1 | sed 's/^/  /'

echo ""
echo "✓ Done: $APP_DIR"
echo ""
echo "Install and run:"
echo "  cp -R '$APP_DIR' /Applications/Said.app"
echo "  open /Applications/Said.app"
