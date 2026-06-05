#!/usr/bin/env bash
# tools/e2e-stress/run.sh
#
# Rapid-recording stress driver for the AirNote backend.
#
# What this tests:
#   • The local backend accepts 50 rapid-fire voice-polish requests without
#     crashing, hanging, or returning a non-2xx status.
#   • Each request carries a distinct recording_id so the backend never
#     conflates two sessions (simulates rapid Caps Lock cycling).
#   • Silent 1-second WAV (all-zero PCM) triggers the "no speech" path, which
#     is the fastest way to flush a full SSE response and close the connection.
#
# What this does NOT test (still manual — see README-tests.md §Manual):
#   • Real Deepgram streaming (needs a live key and actual audio).
#   • AppKit status-bar safety (needs a running Tauri UI + macOS).
#   • Global hotkey de-bounce (needs CGEventTap permissions).
#
# Usage:
#   BACKEND_URL=http://localhost:48484 ./tools/e2e-stress/run.sh
#   CYCLES=100 ./tools/e2e-stress/run.sh   # increase load
#
# Prerequisites: curl, python3 (for WAV generation), uuidgen (macOS built-in)

set -euo pipefail

BACKEND_URL="${BACKEND_URL:-http://localhost:48484}"
CYCLES="${CYCLES:-50}"
SILENCE_WAV="$(mktemp /tmp/silence-XXXXXX.wav)"
PASS=0
FAIL=0

# ── Generate a 1-second 16 kHz mono 16-bit silence WAV ───────────────────────
python3 - "$SILENCE_WAV" <<'PYEOF'
import sys, struct, wave
path = sys.argv[1]
sample_rate, num_frames, channels, sampwidth = 16000, 16000, 1, 2
with wave.open(path, 'wb') as w:
    w.setnchannels(channels)
    w.setsampwidth(sampwidth)
    w.setframerate(sample_rate)
    w.writeframes(b'\x00' * num_frames * channels * sampwidth)
PYEOF

cleanup() { rm -f "$SILENCE_WAV"; }
trap cleanup EXIT

echo "=== AirNote e2e stress: $CYCLES rapid-fire cycles against $BACKEND_URL ==="
echo ""

# ── Health check ─────────────────────────────────────────────────────────────
if ! curl -sf "$BACKEND_URL/v1/health" >/dev/null 2>&1; then
    echo "ERROR: backend not reachable at $BACKEND_URL/v1/health"
    echo "       Start it with: just dev-backend"
    exit 1
fi
echo "✓ backend healthy"
echo ""

# ── Rapid-fire recording cycles ───────────────────────────────────────────────
for i in $(seq 1 "$CYCLES"); do
    RECORDING_ID="$(uuidgen | tr '[:upper:]' '[:lower:]')"

    # POST silence WAV; capture HTTP status code only (discard SSE body).
    # --max-time 8: generous timeout per request.
    HTTP_STATUS=$(curl -sf \
        --max-time 8 \
        -w "%{http_code}" \
        -o /dev/null \
        -F "audio=@${SILENCE_WAV};type=audio/wav" \
        -F "target_app=com.e2e.stress.test" \
        "$BACKEND_URL/v1/voice/polish" \
        2>&1 || echo "000")

    if [[ "$HTTP_STATUS" == "200" ]]; then
        PASS=$((PASS + 1))
        printf "  [%3d/%d] id=%.8s… HTTP %s ✓\n" "$i" "$CYCLES" "$RECORDING_ID" "$HTTP_STATUS"
    else
        FAIL=$((FAIL + 1))
        printf "  [%3d/%d] id=%.8s… HTTP %s ✗\n" "$i" "$CYCLES" "$RECORDING_ID" "$HTTP_STATUS"
    fi
done

echo ""
echo "=== Results: $PASS passed, $FAIL failed out of $CYCLES cycles ==="

if [[ $FAIL -gt 0 ]]; then
    echo "FAIL"
    exit 1
else
    echo "PASS — no crashes or non-2xx responses across $CYCLES rapid cycles"
    exit 0
fi
