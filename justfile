# AirNote — task runner.
# Install: `brew install just` (macOS) or `cargo install just`.
# Run `just` (no args) to see the list.

# Default recipe: print the list.
default:
    @just --list --unsorted

# ── Day-to-day ───────────────────────────────────────────────────────────────

# Build the airnote-backend, sync it to the Tauri sidecar slot, then launch
# the desktop app in dev mode (Vite + Tauri).
dev:
    ./dev.sh

# Enterprise admin stack: rebuild control-plane, start API (:3100) + admin UI (:5174).
# Vite proxies /v1 to the API — always use this instead of starting binaries manually.
dev-admin:
    ./dev-admin.sh

# Local airnote-backend daemon only (no Tauri). Rebuilds before each start.
dev-backend:
    ./dev-backend.sh

# ── Notch HUD sidecar (native Swift, experimental, macOS only) ───────────────

# Build the native Swift notch-HUD sidecar (release) and stage it as a Tauri
# sidecar binary. `cargo tauri dev` finds the .build output directly; the staged
# copy is for bundling. Enable at runtime with AIRNOTE_NOTCH_SIDECAR=1.
notch-build:
    cd desktop/notch-sidecar && swift build -c release
    cp desktop/notch-sidecar/.build/release/AirNoteNotch desktop/src-tauri/binaries/airnote-notch-aarch64-apple-darwin
    @echo "✓ staged airnote-notch — run the app with AIRNOTE_NOTCH_SIDECAR=1"

# Watch the real native notch HUD cycle through every state (no Tauri needed).
notch-demo:
    cd desktop/notch-sidecar && swift build -c release
    ./desktop/notch-sidecar/demo.sh | ./desktop/notch-sidecar/.build/release/AirNoteNotch

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

# Build a signed AirNote.app + DMG for the given target.
# Default target is the host arch on Apple Silicon.
#   just dmg                       # aarch64
#   just dmg x86_64-apple-darwin   # Intel
dmg TARGET="aarch64-apple-darwin":
    ./scripts/build-dmg.sh {{TARGET}}

# Build a local test DMG from the current checkout.
# No version bump, push, deploy, or updater artifacts. Signs + notarizes.
#   just local-dmg
#   just local-dmg x86_64-apple-darwin
local-dmg TARGET="aarch64-apple-darwin":
    ./scripts/build-local-dmg.sh {{TARGET}}

# Tag and push a release. Run `just bump <version>` first, commit the
# changes, then `just release <version>` to fire the Release workflow.
#   just release 0.2.0
release VERSION:
    git tag v{{VERSION}}
    git push origin main --tags

# ── Maintenance ──────────────────────────────────────────────────────────────

# Reset local macOS onboarding/setup state for demo recording.
# Keeps local recordings, meetings, vocabulary, audio, and downloaded STT models.
reset-onboarding:
    ./scripts/reset-local-onboarding.sh

# Wipe build outputs.
clean:
    cargo clean
    rm -rf desktop/dist desktop/node_modules landing/.next landing/node_modules

# Refresh the npm dependency caches both apps need.
install-js:
    cd desktop && npm ci
    cd landing && npm ci

# ── Testing ───────────────────────────────────────────────────────────────────

# Rapid-recording HTTP stress: fires 50 silence-WAV requests against the local
# backend and asserts HTTP 200 for every cycle.  Requires `just dev-backend`
# running in another terminal.  Set CYCLES=N to change the load.
e2e-stress:
    ./tools/e2e-stress/run.sh

# Chaos/longevity soak monitor: attaches to a RUNNING AirNote app that was
# launched in soak mode and asserts it survives + self-heals + does not leak.
# Launch the app first (separate terminal):
#   AIRNOTE_CHAOS=1 AIRNOTE_CHAOS_SOAK=1 AIRNOTE_CHAOS_INTERVAL=15 \
#   AIRNOTE_HEAL_STUCK_SECS=12 just dev
# Then: DURATION=1200 just soak
soak:
    ./tools/e2e-stress/soak.sh
