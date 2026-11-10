#!/usr/bin/env bash
# Fetch the on-device speech-to-text pieces the iOS app bundles:
#   - whisper.cpp iOS framework  -> mobile/ios/Vendor/whisper.xcframework
#   - Clario Hinglish STT (base, q8_0) -> mobile/ios/AirNoteApp/LocalModels/
#   - Silero VAD v5.1.2              -> mobile/ios/AirNoteApp/LocalModels/
#
# The Clario model lives in a private Hugging Face repo, so HF_TOKEN must be set:
#   HF_TOKEN=hf_... mobile/ios/scripts/fetch-local-stt.sh
# Then: cd mobile/ios && xcodegen generate --spec project.yml
set -euo pipefail

WHISPER_VERSION="v1.9.2"
MODEL_REPO="Marquestra/clario-hinglish-stt-base-q8"
MODEL_REVISION="31322d708085867bc4c3bb72b72adf0f04078e8b"
MODEL_SHA256="d60be2560f13caf3dd5e0499c686f62790a6c3ea3e626663c407e3c63ca5f99a"
VAD_URL="https://huggingface.co/ggml-org/whisper-vad/resolve/main/ggml-silero-v5.1.2.bin"

IOS_DIR="$(cd "$(dirname "$0")/.." && pwd)"
MODELS_DIR="$IOS_DIR/AirNoteApp/LocalModels"
VENDOR_DIR="$IOS_DIR/Vendor"
mkdir -p "$MODELS_DIR" "$VENDOR_DIR"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

if [[ ! -d "$VENDOR_DIR/whisper.xcframework" ]]; then
  echo "Downloading whisper.cpp $WHISPER_VERSION xcframework..."
  curl -fsSL -o "$TMP/whisper.zip" \
    "https://github.com/ggml-org/whisper.cpp/releases/download/$WHISPER_VERSION/whisper-$WHISPER_VERSION-xcframework.zip"
  unzip -q "$TMP/whisper.zip" -d "$TMP/xc"
  cp -R "$TMP/xc/build-apple/whisper.xcframework" "$VENDOR_DIR/"
fi

MODEL="$MODELS_DIR/clario-hinglish-stt-base-q8.bin"
if [[ ! -f "$MODEL" ]] || ! echo "$MODEL_SHA256  $MODEL" | shasum -a 256 -c - >/dev/null 2>&1; then
  : "${HF_TOKEN:?Set HF_TOKEN to a Hugging Face token with access to $MODEL_REPO}"
  echo "Downloading $MODEL_REPO@${MODEL_REVISION:0:9}..."
  curl -fsSL -H "Authorization: Bearer $HF_TOKEN" -o "$TMP/model.bin" \
    "https://huggingface.co/$MODEL_REPO/resolve/$MODEL_REVISION/ggml-model-q8_0.bin"
  echo "$MODEL_SHA256  $TMP/model.bin" | shasum -a 256 -c -
  mv "$TMP/model.bin" "$MODEL"
fi

if [[ ! -f "$MODELS_DIR/ggml-silero-v5.1.2.bin" ]]; then
  echo "Downloading Silero VAD..."
  curl -fsSL -o "$MODELS_DIR/ggml-silero-v5.1.2.bin" "$VAD_URL"
fi

echo "On-device STT ready."
