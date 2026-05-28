#!/bin/bash
# One-command meeting simulation launcher.
#
# What it does:
#   1. Ensures control-plane is running (starts it if not)
#   2. Finds the running AirNote desktop app backend (port + secret)
#   3. Logs in all 4 participants
#   4. Creates and starts a meeting
#   5. Opens the simulator in your browser
#
# Usage: bash scripts/simulate.sh

set -e
API="http://localhost:3100"
DIR="$(cd "$(dirname "$0")/.." && pwd)"

echo ""
echo "  ╔═══════════════════════════════════════╗"
echo "  ║   AirNote Meeting Simulator           ║"
echo "  ╚═══════════════════════════════════════╝"
echo ""

# ── Step 1: Check control-plane ──────────────────────────────────────

echo "[1/5] Checking control-plane on :3100..."
if ! curl -s "$API/v1/health" > /dev/null 2>&1; then
  echo "  Starting control-plane (building first)..."
  bash "$DIR/../../scripts/run-control-plane.sh" &
  CP_PID=$!
  # Wait up to 30 seconds for the server to be ready
  for i in $(seq 1 30); do
    if curl -s "$API/v1/health" > /dev/null 2>&1; then
      break
    fi
    sleep 1
  done
  if ! curl -s "$API/v1/health" > /dev/null 2>&1; then
    echo "  ERROR: control-plane failed to start after 30s"
    exit 1
  fi
  echo "  Started (PID $CP_PID)"
else
  echo "  Already running"
fi

# ── Step 2: Find airnote-backend ─────────────────────────────────────

echo "[2/5] Finding AirNote desktop backend..."
BACKEND_PID=$(pgrep -f 'airnote-backend|said-backend' 2>/dev/null | head -1)
if [ -z "$BACKEND_PID" ]; then
  echo "  WARNING: airnote-backend not found. Open the AirNote desktop app first."
  echo "  You can still use the simulator with text-only mode."
  BACKEND_URL=""
  BACKEND_SECRET=""
else
  BACKEND_PORT=$(ps -p "$BACKEND_PID" -o args= | grep -o '\-\-port [0-9]*' | awk '{print $2}')
  BACKEND_SECRET=$(ps eww "$BACKEND_PID" 2>/dev/null | tr ' ' '\n' | grep POLISH_SHARED_SECRET | cut -d= -f2)
  BACKEND_URL="http://127.0.0.1:$BACKEND_PORT"
  echo "  Found: $BACKEND_URL (secret: ${BACKEND_SECRET:0:8}...)"
fi

# ── Step 3: Login all 4 users ────────────────────────────────────────

echo "[3/5] Logging in participants..."

login() {
  local email=$1 pass=$2 label=$3
  local resp
  resp=$(curl -s "$API/v1/auth/login" -H 'Content-Type: application/json' \
    -d "{\"email\":\"$email\",\"password\":\"$pass\"}")
  local token
  token=$(echo "$resp" | python3 -c "import sys,json; print(json.load(sys.stdin)['token'])" 2>/dev/null)
  local aid
  aid=$(echo "$resp" | python3 -c "import sys,json; print(json.load(sys.stdin)['account']['id'])" 2>/dev/null)
  if [ -z "$token" ] || [ "$token" = "None" ]; then
    echo "  ERROR: login failed for $email — $resp"
    exit 1
  fi
  echo "  $label ✓"
  eval "${label}_TOKEN=$token"
  eval "${label}_ID=$aid"
}

login "abhishek@emiactech.com" "vAbhi2678" "ABHI"
login "rahul@emiactech.com" "testpass1234" "RAHUL"
login "priya@emiactech.com" "testpass1234" "PRIYA"
login "anish@emiactech.com" "testpass1234" "ANISH"

# ── Step 4: Create + start meeting ───────────────────────────────────

echo "[4/5] Creating meeting..."
MEETING_RESP=$(curl -s "$API/v1/meetings" \
  -H "Authorization: Bearer $ABHI_TOKEN" \
  -H 'Content-Type: application/json' \
  -d "{\"title\":\"Sprint Planning — Live Simulation\",\"agenda\":\"End-to-end pipeline test with real voice\",\"participant_ids\":[\"$RAHUL_ID\",\"$PRIYA_ID\",\"$ANISH_ID\"]}")

MEETING_ID=$(echo "$MEETING_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin)['meeting']['id'])" 2>/dev/null)
if [ -z "$MEETING_ID" ] || [ "$MEETING_ID" = "None" ]; then
  echo "  ERROR: $MEETING_RESP"
  exit 1
fi

curl -s "$API/v1/meetings/$MEETING_ID/start" \
  -H "Authorization: Bearer $ABHI_TOKEN" -X POST > /dev/null
echo "  Meeting $MEETING_ID — LIVE"

# ── Step 5: Open browser ─────────────────────────────────────────────

SIM_URL="$API/admin/simulator?meeting=$MEETING_ID&t0=$ABHI_TOKEN&t1=$RAHUL_TOKEN&t2=$PRIYA_TOKEN&t3=$ANISH_TOKEN"

echo "[5/5] Opening browser..."
echo ""

if [ -n "$BACKEND_URL" ]; then
  echo "  ┌──────────────────────────────────────────────┐"
  echo "  │ Backend URL:    $BACKEND_URL"
  echo "  │ Backend Secret: $BACKEND_SECRET"
  echo "  │                                              │"
  echo "  │ Paste these into the simulator page fields   │"
  echo "  └──────────────────────────────────────────────┘"
fi

echo ""
open "$SIM_URL" 2>/dev/null || echo "Open: $SIM_URL"
echo "  Done. Speak into the mic to simulate meeting participants."
echo ""
