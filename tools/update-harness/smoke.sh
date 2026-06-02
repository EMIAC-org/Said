#!/usr/bin/env bash
# Manifest smoke test — the exact gate to run in CI before promoting a manifest
# to stable. Catches the nastiest prod failure: Tauri rejects the WHOLE manifest
# if any one platform entry is malformed, silently blocking updates for everyone.
#
# Usage: ./smoke.sh [base-url]   (default http://localhost:3007)
set -euo pipefail
BASE="${1:-http://localhost:${HARNESS_PORT:-3007}}"

fail() { echo "✗ FAIL: $*"; exit 1; }
pass() { echo "✓ $*"; }

echo "── smoke-testing $BASE/latest.json ──"
MAN="$(curl -fsS "$BASE/latest.json")" || fail "manifest not reachable (HTTP error)"
echo "$MAN" | jq . >/dev/null 2>&1 || fail "manifest is not valid JSON"
pass "manifest reachable + valid JSON"

VER="$(echo "$MAN" | jq -r '.version')"
[[ "$VER" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-.+].*)?$ ]] || fail "version '$VER' is not semver"
pass "version is semver: $VER"

# Every platform entry must have a non-empty signature AND a reachable url.
PLATFORMS="$(echo "$MAN" | jq -r '.platforms | keys[]')"
[[ -n "$PLATFORMS" ]] || fail "no platforms in manifest"
while IFS= read -r p; do
  SIG="$(echo "$MAN" | jq -r --arg p "$p" '.platforms[$p].signature')"
  URL="$(echo "$MAN" | jq -r --arg p "$p" '.platforms[$p].url')"
  [[ -n "$SIG" && "$SIG" != "null" ]] || fail "[$p] empty signature"
  [[ -n "$URL" && "$URL" != "null" ]] || fail "[$p] empty url"
  curl -fsI "$URL" >/dev/null 2>&1 || fail "[$p] artifact URL not reachable: $URL"
  pass "[$p] signature present + artifact reachable"
done <<< "$PLATFORMS"

echo "── PASS: manifest is safe to promote ──"
