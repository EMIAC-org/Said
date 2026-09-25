# AGENTS.md
> Shared context for all AI coding assistants — Claude Code, Codex, Cursor, Gemini CLI, etc.
> This file is symlinked as CLAUDE.md.

---

## What This Project Is

**AirNote** is a macOS + Windows voice dictation app that polishes speech in real-time using an LLM.
Hold your hotkey (Fn, Caps Lock or a modifier), speak, release — AirNote transcribes locally, polishes text, and types it into any focused app in English, Hindi, Hinglish, or whatever mix comes out of your mouth.

Core runtime (platform-specific code paths shown):
1. The dictation hotkey (Fn / Globe, Caps Lock, or a single modifier) triggers the `hotkey` crate
   - macOS: `CGEventTap` (Input Monitoring permission)
   - Windows: `WH_KEYBOARD_LL` low-level keyboard hook (no permission required)
2. `recorder` captures audio via `cpal` (CoreAudio on macOS, WASAPI on Windows, 16 kHz PCM)
3. `desktop/src-tauri/src/dictation_stt.rs` runs local whisper.cpp speech recognition
4. `backend /v1/voice/polish` (Axum SSE) takes the local transcript. Polish off: the transcript is
   typed as it is. Polish on: the control-plane sends it to Gemma with a short prompt
   (`said_core::polish::dictation`) plus the user's Dictionary words found in it, and Gemma's reply
   is typed as it is. Nothing runs on the text before or after the model; if the server fails,
   the transcript is typed.
5. SSE tokens update AirNote's preview; `paster` inserts the final output once
   - macOS: `CGEventKeyboardSetUnicodeString` (Accessibility permission)
   - Windows: `SendInput(KEYEVENTF_UNICODE)` (no permission required)
6. An edit watch reads what the user kept of the pasted text. History shows the kept text, and
   small name/term fixes (`air note` → `AirNote`) are added to the Dictionary
   (`store::dictionary::learned_pairs`). The Dictionary is only used by polish.
   - macOS only; Windows falls back to clipboard-only paste (UIAutomation port pending)

---

## Tech Stack

| Layer | Technology |
|---|---|
| Language | Rust 2024 edition (workspace) + TypeScript (React frontend) |
| Desktop shell | Tauri v2 |
| HTTP server | Axum (async, SSE streaming) |
| Database | SQLite via r2d2 + rusqlite (20 migrations, WAL mode) |
| UI | React + Vite + TypeScript |
| STT | Local whisper.cpp (desktop-owned, no cloud STT fallback) |
| LLM polish | Gemma 4 26B A4B on DeepInfra, called by the control-plane |
| Audio capture | cpal — CoreAudio (macOS) / WASAPI (Windows), 16 kHz PCM |
| Global hotkey | CGEventTap (macOS, requires Input Monitoring) / `WH_KEYBOARD_LL` (Windows, no permission) |
| HID typing | CGEventKeyboardSetUnicodeString (macOS, requires Accessibility) / `SendInput(KEYEVENTF_UNICODE)` (Windows, no permission) |
| Telemetry | Sentry (opt-out, env-gated, `rustls` transport) |
| Task runner | just (justfile in repo root) |

---

## Commands

```bash
# Dev mode: builds airnote-backend, syncs sidecar, launches Tauri + Vite
just dev

# UI only, in a browser, no Rust: every Tauri command and HTTP call is mocked
# (desktop/src/dev/mock). ?scenario=ready|new|onboarding|update|signed-out&theme=light
just mock

# Full CI gate — run before every PR
just check              # fmt-check + clippy + tests + typecheck

# Individual gates
just fmt                # fix formatting (cargo fmt --all)
just fmt-check          # check formatting only
just clippy             # clippy warnings
just test               # cargo test --workspace
just typecheck          # cd desktop && npm run typecheck

# Release
just dmg                          # build Apple Silicon DMG
just dmg x86_64-apple-darwin      # build Intel DMG
just bump 2.1.0                   # bump version everywhere
just release 2.1.0                # tag + push (run after bump + commit)

# Cargo only (no Tauri)
cargo build -p said-backend --release
cargo check --workspace           # fast type-check, no codegen

# JS
cd desktop && npm run typecheck
cd desktop && npm ci              # reinstall deps
```

---

## Repository Structure

```
/crates/hotkey        global hotkey listener (CGEventTap): Fn, Caps Lock or a modifier
/crates/recorder      CoreAudio capture at 16 kHz
/crates/core          shared transcript metadata + polish helpers
/crates/paster        HID typing into the focused field + Accessibility reads for the edit watch
/crates/backend       local Axum daemon — dictation route, History, Dictionary, SQLite
/crates/control-plane Fly.io cloud backend (Postgres) — EXCLUDED from workspace
/desktop/src-tauri    Tauri v2 shell — spawns airnote-backend, 39 commands
/desktop/src          React + Vite UI
/scripts              build-dmg.sh, bump-version.sh
/justfile             task runner (just dev, just check, just dmg, etc.)
```

`crates/control-plane` is excluded from the Cargo workspace (postgres vs rusqlite linkage conflict).
Build it standalone: `cd crates/control-plane && cargo build`.

---

## Architecture — Key Files

```
crates/paster/src/lib.rs              type_text() — final HID insertion loop (6ms delays, critical)
crates/backend/src/routes/voice.rs    main SSE endpoint — transcript → (polish) → stream
crates/core/src/polish/dictation.rs   the dictation polish prompt + word list
crates/backend/src/store/dictionary.rs the user's word list: learn from edits, pick words for a transcript
crates/backend/src/routes/history.rs  History, and PUT /v1/recordings/:id/kept (kept text + learning)
crates/backend/src/lib.rs             AppState, prefs cache (30s TTL)
crates/backend/src/store/mod.rs       SQLite pool (r2d2, max 5 connections)
desktop/src-tauri/src/backend.rs      spawns airnote-backend, polls /v1/health, find_binary()
desktop/src-tauri/src/backend_guard.rs  reaps leaked airnote-backend processes
desktop/src-tauri/src/dictation_stt.rs local whisper.cpp dictation STT adapter
desktop/src-tauri/src/main.rs         all 39 Tauri commands, app lifecycle
```

---

## Design Rules (Non-Negotiable)

1. **`just check` must pass before committing** — fmt-check + clippy + tests + typecheck
2. **HID delays are sacred** — `paster/src/lib.rs` has 6ms keydown→keyup + 6ms post-keyup. Removing these causes word-breaking at streaming speeds. Do not touch without understanding the hardware queue saturation root cause.
3. **Shipped sidecar binary name is `airnote-backend` everywhere** — `backend.rs`, `backend_guard.rs`, `tauri.conf.json`, `build-dmg.sh`, and release workflows must agree. The Rust package can remain `said-backend`; the packaged/runtime binary must not ship as `said-backend`.
4. **`control-plane` never re-enters the workspace** — postgres vs rusqlite linker conflict is unfixable without vendoring. Build it standalone.
5. **No after-effects on dictation text** — with polish off the transcript is typed as it is; with polish on the model's reply is typed as it is. Do not add formatters, replacers or guards around them; change the prompt instead.

---

## Environment Variables (`.env` / shell)

```
GATEWAY_API_KEY=           # gateway key for the local text helpers
GEMINI_API_KEY=            # optional — Gemini provider for the local text helpers
POLISH_SHARED_SECRET=      # auto — set by Tauri on spawn, never set manually
```

See `.env.example` for the full list of optional configuration.

---

## Version Roadmap

| Version | Goal | Status |
|---|---|---|
| v1.0 | Voice Polish — basic dictation + polish | Done |
| v2.0 | AirNote rebrand, Hinglish-native, streaming word fix, learning pipeline (replaced by the Dictionary, 2026-09) | Done |
| v2.x | Performance fixes (faster STT fallback, embed circuit breaker, pool tuning) | Planned |
| v3.0 | Windows port (unsigned beta), Sentry telemetry, stable/beta channels, PRIVACY/EULA | In progress |
| v3.x | Windows Authenticode signing, macOS notarization, in-app Settings toggles, manifests-branch beta discovery, UIAutomation tree-reads | Planned |
| v4.0 | Local-only mode (on-device STT + LLM) | Roadmap |
