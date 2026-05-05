# Said — task runner.
# Install: `brew install just` (macOS) or `cargo install just`.
# Run `just` (no args) to see the list.

# Default recipe: print the list.
default:
    @just --list --unsorted

# ── Day-to-day ───────────────────────────────────────────────────────────────

# Build the polish-backend, sync it to the Tauri sidecar slot, then launch
# the desktop app in dev mode (Vite + Tauri).
dev:
    ./dev.sh

# Pretty-print the workspace + control-plane.
fmt:
    cargo fmt --all
    cd crates/control-plane && cargo fmt

# Check everything is formatted (CI does this too).
fmt-check:
    cargo fmt --all --check
    cd crates/control-plane && cargo fmt --check

# Run clippy on the workspace + control-plane separately (control-plane is
# excluded from the workspace because of sqlx vs rusqlite linkage).
clippy:
    cargo clippy --workspace --all-targets
    cd crates/control-plane && cargo clippy --all-targets

# Run all Rust tests.
test:
    cargo test --workspace --all-targets

# Typecheck the desktop and landing apps.
typecheck:
    cd desktop && npm run typecheck
    cd landing && npm run typecheck

# Run every gate CI runs, locally. Hit this before opening a PR.
check: fmt-check clippy test typecheck
    @echo "✓ all gates green"

# ── Release ──────────────────────────────────────────────────────────────────

# Bump the project version everywhere it's pinned.
#   just bump 0.2.0
bump VERSION:
    ./scripts/bump-version.sh {{VERSION}}

# Build a signed Said.app + DMG for the given target.
# Default target is the host arch on Apple Silicon.
#   just dmg                       # aarch64
#   just dmg x86_64-apple-darwin   # Intel
dmg TARGET="aarch64-apple-darwin":
    ./scripts/build-dmg.sh {{TARGET}}

# Tag and push a release. Run `just bump <version>` first, commit the
# changes, then `just release <version>` to fire the Release workflow.
#   just release 0.2.0
release VERSION:
    git tag v{{VERSION}}
    git push origin main --tags

# ── Maintenance ──────────────────────────────────────────────────────────────

# Wipe build outputs.
clean:
    cargo clean
    rm -rf desktop/dist desktop/node_modules landing/.next landing/node_modules

# Refresh the npm dependency caches both apps need.
install-js:
    cd desktop && npm ci
    cd landing && npm ci
