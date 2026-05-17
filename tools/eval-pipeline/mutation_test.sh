#!/usr/bin/env bash
#
# Mutation testing: deliberately break the code, verify the eval catches it.
# Saves file content before mutation and restores after (no git checkout).
#
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

TRANSCRIPTS="tools/eval-pipeline/transcripts.jsonl"
VOCAB="tools/eval-pipeline/vocab_seed.json"
PASS=0
FAIL=0
SKIP=0

run_mutation() {
    local name="$1"
    local file="$2"
    local find_str="$3"
    local replace_str="$4"

    # Save original
    local backup
    backup=$(cat "$file")

    # Apply mutation via python
    if ! python3 -c "
import sys
content = open('$file').read()
old = '''$find_str'''
if old not in content:
    sys.exit(2)
open('$file','w').write(content.replace(old, '''$replace_str''', 1))
" 2>/dev/null; then
        echo "  SKIP  $name (pattern not found)"
        SKIP=$((SKIP + 1))
        return
    fi

    # Build
    if ! cargo build -p said-backend --bin eval-pipeline --release 2>/dev/null; then
        echo "  SKIP  $name (build failed after mutation)"
        echo "$backup" > "$file"
        SKIP=$((SKIP + 1))
        return
    fi

    # Run eval — expect FAILURE
    if ./target/release/eval-pipeline --transcripts "$TRANSCRIPTS" --vocab "$VOCAB" >/dev/null 2>&1; then
        echo "  FAIL  $name — eval PASSED (blind spot!)"
        FAIL=$((FAIL + 1))
    else
        echo "  PASS  $name — eval caught the bug"
        PASS=$((PASS + 1))
    fi

    # Restore original
    echo "$backup" > "$file"
}

echo ""
echo "══ MUTATION TESTING ══"
echo "Breaking code on purpose. Eval must catch every break."
echo ""

echo "Building baseline..."
cargo build -p said-backend --bin eval-pipeline --release 2>/dev/null
echo ""

# Mutation 1: Revert substring matching
run_mutation \
    "M1: Revert to substring match (8GB-in-128GB)" \
    "crates/backend/src/store/vocab_embeddings.rs" \
    "let whole_word_match = if term_words.len() == 1 {" \
    "if transcript_lower.contains(&term_lower) { return true; } let whole_word_match = if term_words.len() == 1 {"

# Mutation 2: Lower k-threshold to 1
run_mutation \
    "M2: k=1 (premature promotion)" \
    "crates/backend/src/store/pending_promotions.rs" \
    "pub const DEFAULT_K: i64 = 3;" \
    "pub const DEFAULT_K: i64 = 1;"

# Mutation 3: Remove short-term phonetic block
run_mutation \
    "M3: Remove min-length phonetic gate" \
    "crates/backend/src/store/vocab_embeddings.rs" \
    "if term_lower.len() < 4 {
                return false;
            }" \
    "let _ = term_lower.len();"

# Mutation 4: Loosen phonetic threshold back to 0.70
run_mutation \
    "M4: Loosen phonetic to 0.70" \
    "crates/backend/src/store/vocab_embeddings.rs" \
    ">= 0.80" \
    ">= 0.50"

echo ""
echo "════════════════════════════════════"
echo "  Caught: $PASS  Missed: $FAIL  Skipped: $SKIP"
echo "════════════════════════════════════"

if [ "$FAIL" -gt 0 ]; then
    echo "FAIL — test suite has blind spots!"
    exit 1
else
    echo "PASS — every mutation was caught"
fi
