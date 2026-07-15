# AGENTS.md
> Shared context for all AI coding assistants — Claude Code, Codex, Cursor, Gemini CLI, etc.
> This file is symlinked as CLAUDE.md.

---

## What This Project Is

**AirNote** is a macOS + Windows voice dictation app that polishes speech in real-time using an LLM.
Hold Caps Lock, speak, release — AirNote transcribes locally, polishes text, and types it into any focused app in English, Hindi, Hinglish, or whatever mix comes out of your mouth.

Core runtime (platform-specific code paths shown):
1. Caps Lock triggers `hotkey` crate
   - macOS: `CGEventTap` (Input Monitoring permission)
   - Windows: `WH_KEYBOARD_LL` low-level keyboard hook (no permission required)
2. `recorder` captures audio via `cpal` (CoreAudio on macOS, WASAPI on Windows, 16 kHz PCM)
3. `desktop/src-tauri/src/dictation_stt.rs` runs local whisper.cpp speech recognition
4. `backend /v1/voice` (Axum SSE) requires the local transcript and polishes it via streaming LLM
5. `script.rs` Devanagari→Roman guard runs after every token (Hinglish guarantee)
6. SSE tokens update AirNote's preview; `paster` inserts the final polished output once
   - macOS: `CGEventKeyboardSetUnicodeString` (Accessibility permission)
   - Windows: `SendInput(KEYEVENTF_UNICODE)` (no permission required)
7. A 30s edit watch classifies corrections (4-way) → validates (3 gates) → persists to SQLite
   - macOS only in v3.0; Windows falls back to clipboard-only paste (UIAutomation port pending)

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
| LLM polish | Groq llama-3.3-70b (primary), OpenAI Codex (fallback) |
| Embeddings | Gemini text-embedding-004 (256-d, stored in SQLite) |
| Edit classifier | Groq llama-3.1-8b-instant (4-way, ~150ms) |
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
/crates/hotkey        global Caps Lock listener (CGEventTap)
/crates/recorder      CoreAudio capture at 16 kHz
/crates/core          shared transcript metadata + polish helpers
/crates/paster        HID typing into focused field + 30s edit watch
/crates/backend       local Axum daemon — STT, LLM, SQLite, learning pipeline
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
crates/backend/src/routes/voice.rs    main SSE endpoint — STT → LLM polish → stream
crates/backend/src/llm/script.rs      80-glyph Devanagari→Roman romanizer
crates/backend/src/llm/classifier.rs  4-way edit classifier
crates/backend/src/llm/promotion_gate.rs  3 hallucination gates
crates/backend/src/llm/prompt.rs      LLM polish prompt text
crates/backend/src/lib.rs             AppState, prefs cache (30s TTL), lexicon cache (60s TTL)
crates/backend/src/store/mod.rs       SQLite pool (r2d2, max 5 connections)
crates/backend/src/embedder/gemini.rs Gemini embedding client
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
5. **STT transcript is NOT ground truth** — the classifier explicitly handles STT+polish agreement on the wrong word. See `classifier.rs:20`. Never assume the transcript is correct.
6. **Lexicon cache needs explicit invalidation** — any route that writes to `corrections` or `stt_replacements` must call `invalidate_lexicon_cache()`. The 60s TTL is not sufficient on its own.

---

## Environment Variables (`.env` / shell)

```
GATEWAY_API_KEY=           # required — Groq / gateway key for LLM polish + classifier
GEMINI_API_KEY=            # optional — embeddings (learning degrades without it)
POLISH_SHARED_SECRET=      # auto — set by Tauri on spawn, never set manually
```

See `.env.example` for the full list of optional configuration.

---

## Version Roadmap

| Version | Goal | Status |
|---|---|---|
| v1.0 | Voice Polish — basic dictation + polish | Done |
| v2.0 | AirNote rebrand, Hinglish-native, streaming word fix, learning pipeline | Done |
| v2.x | Performance fixes (faster STT fallback, embed circuit breaker, pool tuning) | Planned |
| v3.0 | Windows port (unsigned beta), Sentry telemetry, stable/beta channels, PRIVACY/EULA | In progress |
| v3.x | Windows Authenticode signing, macOS notarization, in-app Settings toggles, manifests-branch beta discovery, UIAutomation tree-reads | Planned |
| v4.0 | Local-only mode (on-device STT + LLM) | Roadmap |
