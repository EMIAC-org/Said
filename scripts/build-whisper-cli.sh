#!/usr/bin/env bash
# Build a self-contained `whisper-cli` for bundling into AirNote.app, plus fetch
# the Silero VAD model. Called by build-dmg.sh; can also be run standalone.
#
#   scripts/build-whisper-cli.sh [aarch64-apple-darwin|x86_64-apple-darwin]
#
# Outputs (deterministic paths consumed by build-dmg.sh):
#   target/<triple>/release/whisper-cli           — static, Metal-embedded binary
#   target/whisper-models/ggml-silero-v5.1.2.bin  — VAD model
#
# Design notes:
#   * BUILD_SHARED_LIBS=OFF → ggml/whisper are statically linked, so the single
#     `whisper-cli` file is self-contained (no libwhisper.dylib/libggml.dylib to
#     ship or fix up rpaths for).
#   * GGML_METAL_EMBED_LIBRARY=ON → the Metal shaders are embedded in the binary,
#     so there is no .metallib to bundle alongside it.
#   * Pinned to a VAD-capable whisper.cpp ref (supports `--vad --vad-model`).
#     Override with WHISPER_CPP_REF=<tag/sha>.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="${1:-aarch64-apple-darwin}"
WHISPER_CPP_REF="${WHISPER_CPP_REF:-v1.7.6}"
DEPLOY_TARGET="${MACOSX_DEPLOYMENT_TARGET:-13.0}"

case "$TARGET" in
  aarch64-apple-darwin) OSX_ARCH="arm64"  ;;
  x86_64-apple-darwin)  OSX_ARCH="x86_64" ;;
  *) echo "unsupported target: $TARGET" >&2; exit 1 ;;
esac

bold='\033[1m'; green='\033[0;32m'; red='\033[0;31m'; nc='\033[0m'
step() { echo -e "\n${bold}▶ $*${nc}"; }
ok()   { echo -e "  ${green}✓ $*${nc}"; }
fail() { echo -e "\n  ${red}✗ $*${nc}\n"; exit 1; }

command -v cmake >/dev/null || fail "cmake not found — install with: brew install cmake"
command -v git   >/dev/null || fail "git not found"

SRC_DIR="$REPO_ROOT/target/whisper-cpp-src"
BUILD_DIR="$SRC_DIR/build-$OSX_ARCH"
OUT_BIN="$REPO_ROOT/target/$TARGET/release/whisper-cli"
MODELS_DIR="$REPO_ROOT/target/whisper-models"
SILERO_OUT="$MODELS_DIR/ggml-silero-v5.1.2.bin"

# ── 1. Source checkout (cached, pinned) ──────────────────────────────────────
if [ ! -d "$SRC_DIR/.git" ]; then
  step "Clone whisper.cpp"
  git clone --depth 1 --branch "$WHISPER_CPP_REF" \
    https://github.com/ggml-org/whisper.cpp "$SRC_DIR" \
    || git clone "https://github.com/ggml-org/whisper.cpp" "$SRC_DIR"
fi
step "Checkout whisper.cpp @ $WHISPER_CPP_REF"
git -C "$SRC_DIR" fetch --depth 1 origin "$WHISPER_CPP_REF" 2>/dev/null || true
git -C "$SRC_DIR" checkout -q "$WHISPER_CPP_REF" 2>/dev/null \
  || git -C "$SRC_DIR" checkout -q FETCH_HEAD
ok "source at $(git -C "$SRC_DIR" rev-parse --short HEAD)"

# ── 2. Build (static + Metal embedded, per-arch) ─────────────────────────────
step "Build whisper-cli ($OSX_ARCH, static, Metal embedded)"
cmake -S "$SRC_DIR" -B "$BUILD_DIR" \
  -DCMAKE_BUILD_TYPE=Release \
  -DBUILD_SHARED_LIBS=OFF \
  -DGGML_METAL=ON \
  -DGGML_METAL_EMBED_LIBRARY=ON \
  -DGGML_ACCELERATE=ON \
  -DWHISPER_BUILD_TESTS=OFF \
  -DWHISPER_BUILD_SERVER=OFF \
  -DWHISPER_BUILD_EXAMPLES=ON \
  -DCMAKE_OSX_ARCHITECTURES="$OSX_ARCH" \
  -DCMAKE_OSX_DEPLOYMENT_TARGET="$DEPLOY_TARGET" >/dev/null
cmake --build "$BUILD_DIR" --target whisper-cli --config Release -j "$(sysctl -n hw.ncpu)" >/dev/null

BUILT="$BUILD_DIR/bin/whisper-cli"
[ -x "$BUILT" ] || fail "whisper-cli not produced at $BUILT"
# Verify it's the requested arch and has no non-system dylib deps.
file "$BUILT" | grep -q "$OSX_ARCH" || fail "built whisper-cli is not $OSX_ARCH"
if otool -L "$BUILT" | grep -vE "/usr/lib/|/System/|:$|$BUILT" | grep -q .; then
  echo "  ⚠ non-system dylib dependencies (will need bundling):"
  otool -L "$BUILT" | grep -vE "/usr/lib/|/System/|:$" | sed 's/^/    /'
fi
mkdir -p "$(dirname "$OUT_BIN")"
cp "$BUILT" "$OUT_BIN"
chmod +x "$OUT_BIN"
ok "whisper-cli → $OUT_BIN"

# ── 3. Silero VAD model ──────────────────────────────────────────────────────
mkdir -p "$MODELS_DIR"
if [ ! -f "$SILERO_OUT" ]; then
  step "Download Silero VAD model"
  if [ -f "$SRC_DIR/models/download-vad-model.sh" ]; then
    ( cd "$SRC_DIR" && bash models/download-vad-model.sh silero-v5.1.2 )
    cp "$SRC_DIR/models/ggml-silero-v5.1.2.bin" "$SILERO_OUT"
  else
    curl -fsSL -o "$SILERO_OUT" \
      "https://huggingface.co/ggml-org/whisper-vad/resolve/main/ggml-silero-v5.1.2.bin"
  fi
fi
[ -f "$SILERO_OUT" ] || fail "Silero model missing at $SILERO_OUT"
ok "Silero VAD → $SILERO_OUT"

echo ""
ok "done: whisper-cli + Silero ready for $TARGET"
