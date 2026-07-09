#!/usr/bin/env bash
# Authenticate the local Codex CLI without copying credentials into this repo.
set -euo pipefail

if ! command -v codex >/dev/null 2>&1; then
  echo "Codex CLI is not installed or not on PATH." >&2
  exit 1
fi

echo "Opening Codex device authentication. Complete the browser flow shown by Codex."
codex login --device-auth
echo
codex login status
