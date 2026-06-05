#!/usr/bin/env bash
# Serve the mock update directory over HTTP for the app's updater to poll.
# Mirrors the production endpoint shape: GET /latest.json + GET /<bundle>.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
MOCK="$HERE/mock"
PORT="${HARNESS_PORT:-3007}"
[[ -f "$MOCK/latest.json" ]] || { echo "no manifest — run ./publish.sh first"; exit 1; }
echo "serving $MOCK on http://localhost:${PORT}  (Ctrl-C to stop)"
exec python3 -m http.server "$PORT" --directory "$MOCK"
