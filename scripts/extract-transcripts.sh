#!/bin/bash
# Extract (local_speech_transcript, polished_output) pairs from the running
# desktop's log file. Output is JSONL ready to feed into polish-bench.py.
#
# Usage:
#   scripts/extract-transcripts.sh             # tail the running log
#   scripts/extract-transcripts.sh < my.log    # from a saved log file
#
# Each emitted line:
#   {"name":"r0001","input":"...","prev_output":"...","expect_contains":[],"expect_not":[]}
#
# Fill in expect_contains / expect_not before running polish-bench.py.

set -euo pipefail

LOG="${1:-$HOME/Library/Logs/AirNote/said.log}"

if [ ! -f "$LOG" ]; then
  echo "log file not found: $LOG" >&2
  exit 1
fi

# Use Python to handle Unicode escape sequences (Devanagari is logged as \u{...}).
# We pair the "[finish] ✓ local transcript ready" line (Local speech output)
# with the "[main] polished text:" that follows it (LLM output).
python3 - "$LOG" <<'PY'
import re, sys, json

path = sys.argv[1]
PRE  = re.compile(r"\[finish\] ✓ local transcript ready \([^)]*\): \"(.+)\"\s*$")
POL  = re.compile(r"\[main\] polished text: \"(.+)\"\s*$")

# Python's parser understands \u{...} when we go through unicode-escape, but only
# for \uXXXX form. The Rust tracing log uses \u{XXXX} — translate first.
def deunicode(s: str) -> str:
    return re.sub(
        r"\\u\{([0-9a-fA-F]+)\}",
        lambda m: chr(int(m.group(1), 16)),
        s,
    )

pending_input = None
idx = 0
with open(path, encoding="utf-8", errors="replace") as f:
    for line in f:
        m = PRE.search(line)
        if m:
            pending_input = deunicode(m.group(1))
            continue
        m = POL.search(line)
        if m and pending_input is not None:
            polished = deunicode(m.group(1))
            idx += 1
            print(json.dumps({
                "name": f"r{idx:04d}",
                "input": pending_input,
                "prev_output": polished,
                "expect_contains": [],
                "expect_not": [],
            }, ensure_ascii=False))
            pending_input = None
PY
