#!/usr/bin/env bash
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

TRANSCRIPTS="tools/eval-pipeline/transcripts.jsonl"
VOCAB="tools/eval-pipeline/vocab_seed.json"

# Step 1: Download transcripts if not present
if [ ! -f "$TRANSCRIPTS" ]; then
  echo ">>> Downloading transcripts from HuggingFace..."
  python3 tools/eval-pipeline/download_transcripts.py
fi

LINES=$(wc -l < "$TRANSCRIPTS" | tr -d ' ')
echo ">>> $LINES transcripts ready"

# Step 2: Build the eval binary
echo ">>> Building eval-pipeline..."
cargo build -p said-backend --bin eval-pipeline --release 2>&1 | grep -v "^warning:"

# Step 3: Run
echo ">>> Running eval..."
target/release/eval-pipeline \
  --transcripts "$TRANSCRIPTS" \
  --vocab "$VOCAB" \
  --max-false-injections 50
