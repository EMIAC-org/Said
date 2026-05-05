#!/usr/bin/env bash
# Bump the project version everywhere it's pinned, in one shot.
#
# Usage: scripts/bump-version.sh <new-version>
#   e.g. scripts/bump-version.sh 0.2.0
#
# Updates (in order):
#   1. [workspace.package].version in Cargo.toml          — source of truth
#   2. crates/control-plane/Cargo.toml (excluded from workspace)
#   3. desktop/package.json
#   4. landing/package.json
#   5. desktop/src-tauri/tauri.conf.json
#   6. Cargo.lock                                         — via cargo update -w
#
# After running, review with `git diff` and tag the release:
#   git commit -am "Release v<new-version>" && git tag v<new-version>
#
# The DMG filename in scripts/build-dmg.sh reads VERSION from Cargo.toml at
# build time, so it does not need a manual update.

set -euo pipefail

if [ $# -ne 1 ]; then
  echo "usage: $0 <new-version>" >&2
  echo "  e.g. $0 0.2.0" >&2
  exit 1
fi

NEW="$1"

# Validate semver (major.minor.patch with optional pre-release / build metadata).
if ! [[ "$NEW" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$ ]]; then
  echo "error: '$NEW' is not a valid semver (expected X.Y.Z)" >&2
  exit 1
fi

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# Read current workspace version using the same extractor build-dmg.sh uses.
OLD=$(awk '
  /^\[workspace\.package\]/ { in_section = 1; next }
  /^\[/                     { in_section = 0 }
  in_section && /^[[:space:]]*version[[:space:]]*=/ {
    gsub(/.*=[[:space:]]*"/, "")
    gsub(/".*/, "")
    print
    exit
  }
' Cargo.toml)

if [ -z "$OLD" ]; then
  echo "error: could not read current version from Cargo.toml" >&2
  exit 1
fi

if [ "$OLD" = "$NEW" ]; then
  echo "version already $OLD — nothing to do"
  exit 0
fi

echo "bumping $OLD → $NEW"

# Tiny Python helper for in-place TOML/JSON edits — works on macOS without jq.
py() { python3 - "$@"; }

# 1. Root Cargo.toml — only the version under [workspace.package].
py <<PY
import re, pathlib
p = pathlib.Path("Cargo.toml")
src = p.read_text()
src = re.sub(
    r'(\[workspace\.package\][^\[]*?\nversion\s*=\s*")[^"]+(")',
    rf'\g<1>$NEW\g<2>',
    src,
    count=1,
    flags=re.DOTALL,
)
p.write_text(src)
PY

# 2. control-plane (excluded from workspace, has its own version literal).
py <<PY
import re, pathlib
p = pathlib.Path("crates/control-plane/Cargo.toml")
src = p.read_text()
src = re.sub(
    r'(^\s*version\s*=\s*")[^"]+(")',
    rf'\g<1>$NEW\g<2>',
    src,
    count=1,
    flags=re.MULTILINE,
)
p.write_text(src)
PY

# 3, 4, 5. JSON files — use a targeted regex so we don't reflow the whole file.
# Each of these has a top-level "version" key as the first match in the file,
# so the regex is safe. Validates the result with json.loads after.
for f in desktop/package.json landing/package.json desktop/src-tauri/tauri.conf.json; do
  py <<PY
import re, json, pathlib, sys
p = pathlib.Path("$f")
src = p.read_text()
new_src, n = re.subn(
    r'(^\s*"version"\s*:\s*")[^"]+(")',
    rf'\g<1>$NEW\g<2>',
    src,
    count=1,
    flags=re.MULTILINE,
)
if n != 1:
    sys.exit(f"could not find top-level version in {p}")
json.loads(new_src)  # validate
p.write_text(new_src)
PY
done

# 6. Refresh Cargo.lock so the workspace versions stay in sync. -w restricts
#    the update to workspace members; external dependency versions are untouched.
echo "refreshing Cargo.lock…"
cargo update -w >/dev/null 2>&1 || cargo update --workspace >/dev/null 2>&1 || true

echo
echo "  bumped to $NEW. files changed:"
git diff --stat Cargo.toml Cargo.lock crates/control-plane/Cargo.toml \
                desktop/package.json landing/package.json \
                desktop/src-tauri/tauri.conf.json 2>/dev/null || true
echo
echo "  next:"
echo "    git commit -am 'Release v$NEW'"
echo "    git tag v$NEW"
echo "    git push origin main --tags"
