#!/bin/bash
# Test harness for format_recover — runs test cases through the guard
# and prints input → output for visual verification.
#
# Usage: ./tools/test-format-guard.sh
cd "$(dirname "$0")/.."
cargo test -p said-backend format_guard_harness -- --nocapture 2>&1 | grep -E "^(INPUT|EXPECT|ACTUAL|  ✓|  ✗|---)"
