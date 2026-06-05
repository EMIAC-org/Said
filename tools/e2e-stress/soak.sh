#!/usr/bin/env bash
# tools/e2e-stress/soak.sh
#
# Longevity + chaos soak monitor for the AirNote desktop app.
#
# This does NOT launch the app (the GUI must run with the chaos env vars). It
# attaches to the running process, tortures it via the built-in chaos soak loop,
# and asserts that across the whole run the app:
#   • never dies (PID stays alive the entire duration),
#   • recovers from every injected fault (heal/recover breadcrumbs appear),
#   • does not leak memory (RSS growth stays under a threshold).
#
# ── How to run ────────────────────────────────────────────────────────────────
# Terminal 1 — launch the app in self-torturing soak mode:
#   AIRNOTE_CHAOS=1 AIRNOTE_CHAOS_SOAK=1 \
#   AIRNOTE_CHAOS_INTERVAL=15 AIRNOTE_HEAL_STUCK_SECS=12 \
#   just dev
#   # (or run the built /Applications/AirNote.app binary with the same env)
#
# Terminal 2 — monitor for, say, 20 minutes:
#   DURATION=1200 ./tools/e2e-stress/soak.sh
#
# Exit code 0 = PASS, 1 = FAIL. Prints a summary table at the end.

set -uo pipefail

PROC_NAME="${PROC_NAME:-AirNote}"          # process name to watch (or "said-desktop" in dev)
DURATION="${DURATION:-600}"                 # total soak seconds
SAMPLE_EVERY="${SAMPLE_EVERY:-15}"          # RSS/log sample cadence (s)
RSS_GROWTH_MAX_PCT="${RSS_GROWTH_MAX_PCT:-50}"  # fail if RSS grows more than this %
LOG="${AIRNOTE_LOG:-$HOME/Library/Logs/AirNote/said.log}"

# Markers that prove each recovery path fired at least once. These match the
# said.log wording (NOT the diagnostics breadcrumb-ring strings, which never
# reach the log). macOS ships bash 3.2 — no associative arrays, use plain ones.
NEED_LABELS=("seatbelt" "heal" "chaos")
NEED_PATTERNS=(
  "\[guard\] recovered from panic|caught in guarded callback"
  "\[heal\] stuck .* state reset to idle|\[heal\] processing state stuck"
  "\[chaos\] injecting fault"
)
# Lines that would prove a fault actually escaped (hard abort) — must be absent.
ABORT_PATTERN="fatal runtime|abort\(\)|SIGABRT|process abort"

find_pid() {
  pgrep -x "$PROC_NAME" 2>/dev/null | head -1 \
    || pgrep -f "$PROC_NAME" 2>/dev/null | grep -v "soak.sh" | head -1
}

rss_kb() { ps -o rss= -p "$1" 2>/dev/null | tr -d ' '; }

PID="$(find_pid)"
if [[ -z "${PID:-}" ]]; then
  echo "ERROR: no running '$PROC_NAME' process found."
  echo "       Launch it first with AIRNOTE_CHAOS=1 AIRNOTE_CHAOS_SOAK=1 (see header)."
  exit 1
fi
if [[ ! -f "$LOG" ]]; then
  echo "ERROR: log not found at $LOG (set AIRNOTE_LOG=...)."
  exit 1
fi

echo "=== AirNote soak monitor ==="
echo "  pid=$PID  duration=${DURATION}s  sample=${SAMPLE_EVERY}s  log=$LOG"
echo ""

LOG_START_LINES="$(wc -l < "$LOG" | tr -d ' ')"
RSS_FIRST="$(rss_kb "$PID")"
RSS_PEAK="$RSS_FIRST"
DIED=0
ELAPSED=0

while (( ELAPSED < DURATION )); do
  sleep "$SAMPLE_EVERY"
  ELAPSED=$(( ELAPSED + SAMPLE_EVERY ))

  NOW_PID="$(find_pid)"
  if [[ -z "${NOW_PID:-}" || "$NOW_PID" != "$PID" ]]; then
    echo "  [${ELAPSED}s] ✗ process gone (pid $PID) — app CRASHED, not recovered"
    DIED=1
    break
  fi
  RSS_NOW="$(rss_kb "$PID")"
  [[ -n "$RSS_NOW" && "$RSS_NOW" -gt "$RSS_PEAK" ]] && RSS_PEAK="$RSS_NOW"
  printf "  [%4ds] alive pid=%s rss=%sMB\n" "$ELAPSED" "$PID" "$(( ${RSS_NOW:-0} / 1024 ))"
done

echo ""
echo "=== Assertions ==="
FAIL=0

# 1. Survived the whole run.
if (( DIED == 1 )); then
  echo "  ✗ survival: app process died during soak"
  FAIL=1
else
  echo "  ✓ survival: app alive for full ${DURATION}s"
fi

# 2. Recovery breadcrumbs present in the lines emitted during this run.
NEW_LOG="$(tail -n +"$(( LOG_START_LINES + 1 ))" "$LOG" 2>/dev/null || true)"
i=0
while (( i < ${#NEED_LABELS[@]} )); do
  key="${NEED_LABELS[$i]}"
  pat="${NEED_PATTERNS[$i]}"
  if echo "$NEW_LOG" | grep -Eq "$pat"; then
    echo "  ✓ $key: matched /$pat/"
  else
    echo "  ✗ $key: NO match for /$pat/ (recovery path never fired)"
    FAIL=1
  fi
  i=$(( i + 1 ))
done

# 2b. No fault may have escaped into a real abort.
if echo "$NEW_LOG" | grep -Eq "$ABORT_PATTERN"; then
  echo "  ✗ no-abort: found a hard abort signature — a fault escaped recovery"
  FAIL=1
else
  echo "  ✓ no-abort: no hard abort signatures in the log"
fi

# 3. Memory growth bounded.
if [[ -n "${RSS_FIRST:-}" && -n "${RSS_PEAK:-}" && "$RSS_FIRST" -gt 0 ]]; then
  GROWTH=$(( (RSS_PEAK - RSS_FIRST) * 100 / RSS_FIRST ))
  echo "  rss: start=$(( RSS_FIRST / 1024 ))MB peak=$(( RSS_PEAK / 1024 ))MB growth=${GROWTH}%"
  if (( GROWTH > RSS_GROWTH_MAX_PCT )); then
    echo "  ✗ memory: grew ${GROWTH}% (> ${RSS_GROWTH_MAX_PCT}% budget) — possible leak"
    FAIL=1
  else
    echo "  ✓ memory: growth ${GROWTH}% within ${RSS_GROWTH_MAX_PCT}% budget"
  fi
fi

echo ""
if (( FAIL == 0 )); then
  echo "PASS — app tortured for ${DURATION}s and self-healed every fault, no leak."
  exit 0
else
  echo "FAIL — see assertions above."
  exit 1
fi
