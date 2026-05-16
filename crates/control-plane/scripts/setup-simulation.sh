#!/bin/bash
# Setup a meeting simulation with 4 participants.
# Requires the control-plane server running on localhost:3100.
#
# Usage: bash scripts/setup-simulation.sh

set -e
API="http://localhost:3100"

echo "=== Said Meeting Simulator Setup ==="
echo ""

login() {
  local email=$1 pass=$2 label=$3
  echo "Logging in $label..."
  local resp
  resp=$(curl -s "$API/v1/auth/login" -H 'Content-Type: application/json' \
    -d "{\"email\":\"$email\",\"password\":\"$pass\"}")

  local token
  token=$(echo "$resp" | python3 -c "import sys,json; print(json.load(sys.stdin)['token'])" 2>/dev/null)
  local aid
  aid=$(echo "$resp" | python3 -c "import sys,json; print(json.load(sys.stdin)['account']['id'])" 2>/dev/null)

  if [ -z "$token" ] || [ "$token" = "None" ]; then
    echo "  ERROR: login failed for $email"
    echo "  Response: $resp"
    exit 1
  fi
  echo "  OK (${token:0:8}...)"
  # Export to caller via global vars
  eval "${label}_TOKEN=$token"
  eval "${label}_ID=$aid"
}

login "abhishek@emiactech.com" "vAbhi2678" "ABHI"
login "rahul@emiactech.com" "testpass1234" "RAHUL"
login "priya@emiactech.com" "testpass1234" "PRIYA"
login "anish@emiactech.com" "testpass1234" "ANISH"

echo ""
echo "Creating meeting..."
MEETING_RESP=$(curl -s "$API/v1/meetings" \
  -H "Authorization: Bearer $ABHI_TOKEN" \
  -H 'Content-Type: application/json' \
  -d "{\"title\":\"Sprint Planning — Simulation\",\"agenda\":\"Test the full meeting pipeline end-to-end\",\"participant_ids\":[\"$RAHUL_ID\",\"$PRIYA_ID\",\"$ANISH_ID\"]}")

MEETING_ID=$(echo "$MEETING_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin)['meeting']['id'])" 2>/dev/null)
if [ -z "$MEETING_ID" ] || [ "$MEETING_ID" = "None" ]; then
  echo "  ERROR: create meeting failed"
  echo "  Response: $MEETING_RESP"
  exit 1
fi
echo "  Meeting ID: $MEETING_ID"

echo "Starting meeting..."
START_RESP=$(curl -s "$API/v1/meetings/$MEETING_ID/start" \
  -H "Authorization: Bearer $ABHI_TOKEN" \
  -X POST)
echo "  $START_RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); print('  Status:', d.get('meeting',{}).get('status','?'))" 2>/dev/null || echo "  OK"

echo ""
echo "=========================================="
echo "  OPEN THIS URL IN YOUR BROWSER:"
echo ""
echo "  $API/admin/simulator?meeting=$MEETING_ID&t0=$ABHI_TOKEN&t1=$RAHUL_TOKEN&t2=$PRIYA_TOKEN&t3=$ANISH_TOKEN"
echo ""
echo "=========================================="
