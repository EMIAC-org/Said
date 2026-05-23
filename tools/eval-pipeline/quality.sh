#!/bin/bash
# Run the full-pipeline quality test suite.
# Requires GROQ_API_KEY or GATEWAY_API_KEY in environment.
#
# Usage:
#   ./tools/eval-pipeline/quality.sh
#
set -euo pipefail
cd "$(dirname "$0")/../.."

if [ -z "${GROQ_API_KEY:-${GATEWAY_API_KEY:-}}" ]; then
    echo "ERROR: set GROQ_API_KEY or GATEWAY_API_KEY"
    exit 1
fi

cargo run -p said-backend --bin pipeline-quality 2>/dev/null
