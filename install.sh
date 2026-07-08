#!/bin/bash
# ══════════════════════════════════════════════════════════════════════════════
#  Said — Installer
#
#  Install (first time or update):
#    curl -fsSL https://raw.githubusercontent.com/EMIAC-org/Said/main/install.sh | bash
#
#  After install, manage with:
#    said             → start
#    said stop        → stop
#    said update      → get latest version
#    said status      → check if running
#    said logs        → live logs
#    said errors      → show recent errors
#    said delete      → remove everything
#
#  `vp` is kept as a backwards-compatible shim — old muscle memory still
#  works, but it prints a one-line deprecation hint and forwards to `said`.
# ══════════════════════════════════════════════════════════════════════════════

INSTALL_URL="https://raw.githubusercontent.com/EMIAC-org/Said/main/install.sh"
REPO="EMIAC-org/Said"

INSTALL_DIR="$HOME/Said"
APP_BUNDLE="$INSTALL_DIR/Said.app"
APP_EXEC="$APP_BUNDLE/Contents/MacOS/Said"
PLIST_NAME="com.emiac.said"
PLIST_PATH="$HOME/Library/LaunchAgents/$PLIST_NAME.plist"
LOG_OUT="/tmp/said.log"
LOG_ERR="/tmp/said.err"

# v1 install paths — used only by the migration block below.
LEGACY_INSTALL_DIR="$HOME/VoicePolish"
LEGACY_APP_BUNDLE="$LEGACY_INSTALL_DIR/VoicePolish.app"
LEGACY_PLIST_NAME="com.voicepolish.app"
LEGACY_PLIST_PATH="$HOME/Library/LaunchAgents/$LEGACY_PLIST_NAME.plist"

# ─────────────────────────────────────────────────────────────────────────────
BOLD='\033[1m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; RED='\033[0;31m'; CYAN='\033[0;36m'; NC='\033[0m'

ok()   { echo -e "  ${GREEN}✓ $1${NC}"; }
skip() { echo -e "  ${GREEN}✓ $1 — already done, skipping${NC}"; }
info() { echo -e "  ${YELLOW}→ $1${NC}"; }
fail() { echo -e "\n  ${RED}✗ ERROR: $1${NC}\n"; exit 1; }
step() { echo -e "\n${BOLD}[$1]${NC} $2"; }
note() { echo -e "  ${CYAN}ℹ $1${NC}"; }

echo ""
echo -e "${BOLD}🎤  Said — Setup${NC}"
echo "══════════════════════════════════════════════"

# ── 0. v1 → v2 migration ─────────────────────────────────────────────────────
# v1 shipped under the "Voice Polish" name with bundle id com.voicepolish.app
# in ~/VoicePolish. The v2.0.0 rebrand changed the bundle id, install dir,
# command name, plist label, and log paths. macOS keys TCC permissions and
# Keychain entries by bundle id, so the new install starts from a clean
# slate — there is no in-place upgrade.
if [ -d "$LEGACY_INSTALL_DIR" ]; then
    step "0/5" "Migrating from v1 (Voice Polish)"
    echo ""
    echo -e "  ${YELLOW}${BOLD}Detected a v1 install at $LEGACY_INSTALL_DIR.${NC}"
    echo ""
    echo -e "  v2.0.0 is a rebrand: ${BOLD}Voice Polish → Said${NC}."
    echo "  macOS will treat the new app as a separate identity, so:"
    echo ""
    echo -e "    • ${BOLD}You'll re-grant${NC} Input Monitoring + Accessibility (TCC keys"
    echo -e "      permissions by bundle id, and the bundle id changed)."
    echo -e "    • ${BOLD}You'll re-check${NC} the local speech model in the desktop app"
    echo -e "      and reconnect ChatGPT (\`said auth\`) — Keychain entries are"
    echo -e "      also bundle-id-namespaced."
    echo ""
    echo -e "  This installer will stop the v1 LaunchAgent and remove the v1"
    echo -e "  install dir, then proceed with the v2 install."
    echo ""

    pkill -f "VoicePolish.app/Contents/MacOS"            2>/dev/null || true
    launchctl bootout "gui/$(id -u)/$LEGACY_PLIST_NAME"  2>/dev/null || true
    rm -f "$LEGACY_PLIST_PATH"
    rm -rf "$LEGACY_INSTALL_DIR"
    rm -f /tmp/voice-polish.lock /tmp/voice-polish.log /tmp/voice-polish.err
    rm -f "$HOME/bin/vp" 2>/dev/null || true   # old vp shim — replaced below
    ok "v1 install removed"
fi

# ── 1. Stop any running v2 instance ──────────────────────────────────────────
step "1/5" "Stopping any running instance"
pkill -f "Said/said"                          2>/dev/null || true
pkill -f "Said.app/Contents/MacOS"            2>/dev/null || true
launchctl bootout "gui/$(id -u)/$PLIST_NAME"  2>/dev/null || true
sleep 1
ok "Ready"

# ── 2. Download binary ──────────────────────────────────────────────────────
step "2/5" "Downloading Said"
mkdir -p "$INSTALL_DIR"
mkdir -p "$APP_BUNDLE/Contents/MacOS"
mkdir -p "$APP_BUNDLE/Contents/Resources"

ARCH=$(uname -m)
case "$ARCH" in
    arm64|aarch64) ASSET_NAME="said-aarch64-apple-darwin" ;;
    x86_64)        ASSET_NAME="said-x86_64-apple-darwin"  ;;
    *)             fail "Unsupported architecture: $ARCH" ;;
esac

info "Downloading latest release for $ARCH …"

TAG=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
    | grep '"tag_name"' | head -1 | cut -d'"' -f4)

[ -z "$TAG" ] && fail "Could not find latest release — check https://github.com/$REPO/releases"

DOWNLOAD_URL="https://github.com/$REPO/releases/download/$TAG/$ASSET_NAME"
curl -fsSL -o "$APP_EXEC" "$DOWNLOAD_URL" \
    || fail "Download failed — check https://github.com/$REPO/releases"
chmod +x "$APP_EXEC"

# Remove any stale standalone binary from older v2 installs.
rm -f "$INSTALL_DIR/said"

ok "Binary downloaded $(du -h "$APP_EXEC" | cut -f1 | xargs) — tag $TAG"

# ── 3. Standalone config ────────────────────────────────────────────────────
step "3/5" "Standalone config"
note "This standalone build uses the local speech model + OpenAI OAuth token locally"
note "No shared app DB and no gateway API key are used"
ok "Config flow ready"

# ── 4. .app bundle ──────────────────────────────────────────────────────────
step "4/5" "Configuring .app bundle"

cat > "$APP_BUNDLE/Contents/Info.plist" << 'INFOPLIST'
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
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>2.0</string>
  <key>CFBundleVersion</key>
  <string>1</string>
  <key>LSUIElement</key>
  <true/>
  <key>NSMicrophoneUsageDescription</key>
  <string>Said needs microphone access to record and transcribe your voice.</string>
  <key>NSAccessibilityUsageDescription</key>
  <string>Said needs Accessibility access to paste transcribed text at your cursor.</string>
  <key>NSInputMonitoringUsageDescription</key>
  <string>Said needs Input Monitoring access to detect the fn+Shift hotkey.</string>
</dict>
</plist>
INFOPLIST

# Clear quarantine flag so macOS doesn't block the unsigned binary
xattr -cr "$APP_BUNDLE" 2>/dev/null || true

# Ad-hoc code-sign the bundle.
# Without a signature, TCC (Privacy permissions) tracks the binary by its hash.
# That means every "said update" changes the hash and macOS silently revokes
# Input Monitoring + Accessibility — making the app appear broken after updates.
# An ad-hoc signature (-) makes TCC track by bundle ID (com.emiac.said)
# so permissions survive future updates.
if command -v codesign &>/dev/null; then
    codesign --force --deep --sign - "$APP_BUNDLE" 2>/dev/null && \
        ok "Bundle signed (ad-hoc) — permissions will survive future updates" || \
        note "codesign failed (non-fatal) — permissions may need re-granting after updates"
else
    note "codesign not found — install Xcode CLI tools to avoid re-granting permissions after updates"
fi

# Register the bundle with Launch Services so it gets a proper icon in System Settings
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister \
    -f "$APP_BUNDLE" 2>/dev/null || true

ok ".app bundle ready"

# ── 5. said command + LaunchAgent ───────────────────────────────────────────
step "5/5" "Installing said command + auto-start"

mkdir -p "$HOME/Library/LaunchAgents"
cat > "$PLIST_PATH" << PLEOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>${PLIST_NAME}</string>
  <key>ProgramArguments</key>
  <array><string>${APP_EXEC}</string></array>
  <key>WorkingDirectory</key><string>${INSTALL_DIR}</string>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><false/>
  <key>StandardOutPath</key><string>${LOG_OUT}</string>
  <key>StandardErrorPath</key><string>${LOG_ERR}</string>
</dict></plist>
PLEOF

launchctl bootstrap "gui/$(id -u)" "$PLIST_PATH" 2>/dev/null || \
launchctl load      "$PLIST_PATH"                 2>/dev/null || true
ok "Auto-start at login registered"

mkdir -p "$HOME/bin"
cat > "$HOME/bin/said" << 'SAIDEOF'
#!/bin/bash
INSTALL_DIR="$HOME/Said"
APP_BUNDLE="$INSTALL_DIR/Said.app"
APP_EXEC="$APP_BUNDLE/Contents/MacOS/Said"
PLIST_NAME="com.emiac.said"
PLIST_PATH="$HOME/Library/LaunchAgents/$PLIST_NAME.plist"
INSTALL_URL="https://raw.githubusercontent.com/EMIAC-org/Said/main/install.sh"
LOG_OUT="/tmp/said.log"
LOG_ERR="/tmp/said.err"

_launch() {
  # Always start via LaunchAgent so stdout/stderr go to the log files.
  # open -a bypasses the LaunchAgent plist and logs nothing — never use it.
  : > "$LOG_OUT"
  : > "$LOG_ERR"
  launchctl bootout "gui/$(id -u)/$PLIST_NAME" 2>/dev/null || true
  pkill -f "Said.app/Contents/MacOS" 2>/dev/null || true
  sleep 0.5
  launchctl bootstrap "gui/$(id -u)" "$PLIST_PATH" 2>/dev/null || \
    launchctl load "$PLIST_PATH" 2>/dev/null || true
  sleep 2
}

case "${1:-}" in
  start|"")
    if pgrep -f "Said.app/Contents/MacOS" &>/dev/null; then
      echo "✅  Already running — look for ● in menu bar"
      echo "   (run 'said stop && said' to restart)"
    else
      echo "→  Starting…"
      _launch
      if pgrep -f "Said.app/Contents/MacOS" &>/dev/null; then
        echo "✅  Said started — look for ● in menu bar"
      else
        echo "❌  Failed to start. Errors:"
        echo "──────────────────────────────"
        cat "$LOG_ERR" 2>/dev/null || echo "(no error log)"
        echo "──────────────────────────────"
      fi
    fi
    ;;
  stop)
    launchctl bootout "gui/$(id -u)/$PLIST_NAME" 2>/dev/null || true
    pkill -f "Said.app/Contents/MacOS" 2>/dev/null || true
    rm -f /tmp/said.lock
    echo "⏹   Stopped"
    ;;
  restart)
    echo "→  Restarting…"
    _launch
    if pgrep -f "Said.app/Contents/MacOS" &>/dev/null; then
      echo "✅  Restarted — look for ● in menu bar"
    else
      echo "❌  Failed. Run: said doctor"
    fi
    ;;
  update)
    echo "→  Fetching latest version…"
    curl -fsSL "$INSTALL_URL" | bash
    ;;
  status)
    if pgrep -f "Said.app/Contents/MacOS" &>/dev/null; then
      echo "● Running  (pid $(pgrep -f 'Said.app/Contents/MacOS' | head -1))"
    else
      echo "○ Stopped"
    fi
    if [ -x "$APP_EXEC" ]; then
      echo ""
      "$APP_EXEC" status
    fi
    ;;
  auth)
    "$APP_EXEC" auth
    ;;
  disconnect-openai)
    "$APP_EXEC" disconnect-openai
    ;;
  logs)
    echo "── stdout ($LOG_OUT) ──"
    tail -40 "$LOG_OUT" 2>/dev/null || echo "(empty)"
    ;;
  errors)
    echo "── stderr ($LOG_ERR) ──"
    if [ -s "$LOG_ERR" ]; then
      cat "$LOG_ERR"
    else
      echo "(no errors — good!)"
    fi
    ;;
  doctor)
    echo ""
    echo "🩺  Said — diagnostics"
    echo "──────────────────────────────────────────"
    if pgrep -f "Said.app/Contents/MacOS" &>/dev/null; then
      echo "  Process    : ✅ running (pid $(pgrep -f 'Said.app/Contents/MacOS' | head -1))"
    else
      echo "  Process    : ❌ NOT running  →  run: said"
    fi
    if [ -x "$APP_EXEC" ]; then
      echo "  Binary     : ✅ $APP_EXEC"
    else
      echo "  Binary     : ❌ not found  →  run: said update"
    fi
    if [ -f "$PLIST_PATH" ]; then
      echo "  LaunchAgent: ✅ registered"
    else
      echo "  LaunchAgent: ❌ missing  →  run: said update"
    fi
    echo ""
    echo "  Recent errors:"
    echo "  ──────────────"
    if [ -s "$LOG_ERR" ]; then
      sed 's/^/  /' "$LOG_ERR" | tail -20
    else
      echo "  (none)"
    fi
    echo ""
    echo "  Recent output:"
    echo "  ──────────────"
    grep -E "hotkey|paste|startup|preflight" "$LOG_OUT" 2>/dev/null | tail -10 | sed 's/^/  /' \
      || echo "  (none)"
    echo ""
    echo "  If hotkey or paste says NOT granted, run:"
    echo "    said stop && said"
    echo "  Then grant permissions in System Settings and run:"
    echo "    said restart"
    echo ""
    ;;
  permissions)
    echo ""
    echo "  Binary to add in BOTH permission pages:"
    echo "  $APP_EXEC"
    echo ""
    echo "  In each page: click + → press Cmd+Shift+G → paste the path above"
    echo "  → select Said → Open → toggle ON"
    echo ""
    open "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent"
    sleep 1
    open "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
    echo "  Both System Settings pages are now open."
    echo "  After granting both, run:  said restart"
    echo ""
    ;;
  delete)
    echo "→  Removing Said completely…"
    pkill -f "Said.app/Contents/MacOS" 2>/dev/null || true
    launchctl bootout "gui/$(id -u)/$PLIST_NAME" 2>/dev/null || true
    rm -f "$PLIST_PATH"
    rm -rf "$INSTALL_DIR"
    rm -f "$HOME/bin/said" "$HOME/bin/vp"
    rm -f /tmp/said.lock /tmp/said.log /tmp/said.err
    echo "✓  Done. To reinstall: curl -fsSL $INSTALL_URL | bash"
    ;;
  *)
    echo ""
    echo "  said                start"
    echo "  said stop           stop"
    echo "  said restart        stop + start (use after granting permissions)"
    echo "  said status         is it running?"
    echo "  said auth           connect ChatGPT OAuth"
    echo "  said disconnect-openai  clear the saved OpenAI token"
    echo "  said logs           recent output"
    echo "  said errors         recent errors"
    echo "  said doctor         full diagnostics"
    echo "  said permissions    open System Settings + show exact binary path"
    echo "  said update         download latest version"
    echo "  said delete         remove everything"
    echo ""
    ;;
esac
SAIDEOF
chmod +x "$HOME/bin/said"

# Backwards-compatible `vp` shim: forwards every invocation to `said`,
# printing a one-line deprecation hint on the first arg position.
cat > "$HOME/bin/vp" << 'VPEOF'
#!/bin/bash
echo "  ℹ  'vp' has been renamed to 'said' — running 'said $*' for you" >&2
exec "$HOME/bin/said" "$@"
VPEOF
chmod +x "$HOME/bin/vp"

export PATH="$HOME/bin:$PATH"

for PROFILE in "$HOME/.zshrc" "$HOME/.bash_profile"; do
    if [ -f "$PROFILE" ] && ! grep -q 'HOME/bin' "$PROFILE" 2>/dev/null; then
        echo 'export PATH="$HOME/bin:$PATH"' >> "$PROFILE"
    fi
done
ok "said command installed (with vp compat shim)"

# ── Permission instructions ───────────────────────────────────────────────────
echo ""
echo "══════════════════════════════════════════════"
echo -e "${YELLOW}${BOLD}⚠️  Setup required before first run${NC}"
echo "══════════════════════════════════════════════"
echo ""
echo -e "  ${BOLD}1.${NC} Install or verify the local speech model in the desktop app."
echo ""
echo -e "  ${BOLD}2.${NC} Connect ChatGPT OAuth:"
echo -e "     ${CYAN}${BOLD}said auth${NC}"
echo ""
echo -e "  ${BOLD}3.${NC} Open the required macOS permission panes:"
echo -e "     ${CYAN}${BOLD}said permissions${NC}"
echo ""
echo -e "  ${BOLD}4.${NC} Start Said:"
echo -e "     ${CYAN}${BOLD}said${NC}"
echo ""
echo -e "  Permissions needed:"
echo ""
echo -e "  ${BOLD}• Input Monitoring${NC}  (for Caps Lock hold-to-record)"
echo -e "     System Settings → Privacy & Security → Input Monitoring"
echo -e "     Find ${BOLD}Said${NC} → toggle ${BOLD}ON${NC}"
echo ""
echo -e "  ${BOLD}• Accessibility${NC}  (to paste text at your cursor)"
echo -e "     System Settings → Privacy & Security → Accessibility"
echo -e "     Find ${BOLD}Said${NC} → toggle ${BOLD}ON${NC}"
echo ""
echo -e "  ${BOLD}• Microphone${NC}  (auto-prompted on first recording)"
echo -e "     Just say Allow when the popup appears."
echo ""
echo "══════════════════════════════════════════════"
echo -e "${GREEN}${BOLD}✅  Done!${NC}"
echo ""
echo -e "  ${BOLD}Hotkey:${NC}  Hold Caps Lock to record, release to polish"
echo -e "  ${BOLD}Manage:${NC}  type ${CYAN}said${NC} in Terminal for all commands"
echo ""
