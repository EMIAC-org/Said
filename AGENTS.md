# AGENTS.md
> Shared context for all AI coding assistants — Claude Code, Codex, Cursor, Gemini CLI, etc.
> This file is symlinked as CLAUDE.md.

---

## What This Project Is

**Said** is a macOS voice dictation app that polishes speech in real-time using an LLM.
Hold Caps Lock, speak, release — Said types polished text into any focused app in English,
Hindi, Hinglish, or whatever mix comes out of your mouth.

Core runtime:
1. Caps Lock triggers `hotkey` crate (CGEventTap)
2. `recorder` captures CoreAudio PCM at 16 kHz
3. `core/dg_stream` streams audio to Deepgram nova-3 (pre-warmed WebSocket)
4. `backend /v1/voice` (Axum SSE) polishes the transcript via Groq streaming LLM
5. `script.rs` Devanagari→Roman guard runs after every token (Hinglish guarantee)
6. `paster` types token-by-token into the focused app via Accessibility API
7. A 30s edit watch classifies corrections (4-way) → validates (3 gates) → persists to SQLite

---

## Tech Stack

| Layer | Technology |
|---|---|
| Language | Rust 2024 edition (workspace) + TypeScript (React frontend) |
| Desktop shell | Tauri v2 |
| HTTP server | Axum (async, SSE streaming) |
| Database | SQLite via r2d2 + rusqlite (20 migrations, WAL mode) |
| UI | React + Vite + TypeScript |
| STT | Deepgram nova-3 (WebSocket streaming + batch fallback) |
| LLM polish | Groq llama-3.3-70b (primary), OpenAI Codex (fallback) |
| Embeddings | Gemini text-embedding-004 (256-d, stored in SQLite) |
| Edit classifier | Groq llama-3.1-8b-instant (4-way, ~150ms) |
| Audio capture | CoreAudio (macOS native, 16 kHz PCM) |
| Global hotkey | CGEventTap (macOS Accessibility framework) |
| HID typing | CGEventKeyboardSetUnicodeString (Accessibility API) |
| Task runner | just (justfile in repo root) |

---

## Commands

```bash
# Dev mode: builds said-backend, syncs sidecar, launches Tauri + Vite
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
cargo build -p said --release     # standalone CLI
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
/crates/core          Deepgram WebSocket client + pre-warm logic
/crates/paster        HID typing into focused field + 30s edit watch
/crates/backend       local Axum daemon — STT, LLM, SQLite, learning pipeline
/crates/said          standalone CLI binary
/crates/control-plane Fly.io cloud backend (Postgres) — EXCLUDED from workspace
/desktop/src-tauri    Tauri v2 shell — spawns said-backend, 39 commands
/desktop/src          React + Vite UI
/scripts              build-dmg.sh, bump-version.sh
/justfile             task runner (just dev, just check, just dmg, etc.)
```

`crates/control-plane` is excluded from the Cargo workspace (postgres vs rusqlite linkage conflict).
Build it standalone: `cd crates/control-plane && cargo build`.

---

## Architecture — Key Files

```
crates/paster/src/lib.rs              type_text() — HID typing loop (6ms delays, critical)
crates/backend/src/routes/voice.rs    main SSE endpoint — STT → LLM polish → stream
crates/backend/src/llm/script.rs      80-glyph Devanagari→Roman romanizer
crates/backend/src/llm/classifier.rs  4-way edit classifier
crates/backend/src/llm/promotion_gate.rs  3 hallucination gates
crates/backend/src/llm/prompt.rs      LLM polish prompt text
crates/backend/src/lib.rs             AppState, prefs cache (30s TTL), lexicon cache (60s TTL)
crates/backend/src/store/mod.rs       SQLite pool (r2d2, max 5 connections)
crates/backend/src/stt/deepgram.rs    Deepgram batch STT client (30s timeout)
crates/backend/src/embedder/gemini.rs Gemini embedding client
desktop/src-tauri/src/backend.rs      spawns said-backend, polls /v1/health, find_binary()
desktop/src-tauri/src/backend_guard.rs  reaps leaked said-backend processes
desktop/src-tauri/src/dg_stream.rs    pre-warm Deepgram WS (PREWARM_MAX_AGE = 45s)
desktop/src-tauri/src/main.rs         all 39 Tauri commands, app lifecycle
```

---

## Design Rules (Non-Negotiable)

1. **`just check` must pass before committing** — fmt-check + clippy + tests + typecheck
2. **HID delays are sacred** — `paster/src/lib.rs` has 6ms keydown→keyup + 6ms post-keyup. Removing these causes word-breaking at streaming speeds. Do not touch without understanding the hardware queue saturation root cause.
3. **Binary name is `said-backend` everywhere** — `backend.rs`, `backend_guard.rs`, `tauri.conf.json`, `build-dmg.sh`. Never revert to `polish-backend`.
4. **`control-plane` never re-enters the workspace** — postgres vs rusqlite linker conflict is unfixable without vendoring. Build it standalone.
5. **STT transcript is NOT ground truth** — the classifier explicitly handles STT+polish agreement on the wrong word. See `classifier.rs:20`. Never assume the transcript is correct.
6. **Lexicon cache needs explicit invalidation** — any route that writes to `corrections` or `stt_replacements` must call `invalidate_lexicon_cache()`. The 60s TTL is not sufficient on its own.
7. **Wiki before code** — at session start, fetch the Updates page. At session end, update it. Non-negotiable.

---

## Environment Variables (`.env` / shell)

```
DEEPGRAM_API_KEY=          # required — Deepgram STT (free tier sufficient)
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
| v2.0 | Said rebrand, Hinglish-native, streaming word fix, learning pipeline | Done |
| v2.x | Performance fixes (faster STT fallback, embed circuit breaker, pool tuning) | Planned |
| v3.0 | Local-only mode (on-device STT + LLM) | Roadmap |

---

## Shipyard — Coordination System

Wiki is the source of truth. Two devs: Dev A (Abhishek) and Dev B (Rahul). All plans, progress, and blockers live in the Lark Wiki — not in local files, not in memory.

### Team

| Dev | Name | Focus | Identity |
|---|---|---|---|
| Dev A | Abhishek | sole active developer | A |
| Dev B | Anish Suman (anish877) | — (not active yet) | B |

> Note: Only Dev A (Abhishek) is currently active on this project.

### Wiki — Source of Truth

All project documentation lives in the **Lark Wiki** under `Tech Hub > 02 — Internal Projects > Said`.
Use `lark-cli` to read and write wiki pages. Do NOT maintain separate local markdown files for plans or progress — the wiki is canonical.

**Wiki structure:**
```
Said
├── Overview                   (project summary, status, quick start)
├── Architecture & Tech Stack  (crates, data flow, external services, key files)
├── URLs & Access              (endpoints, API keys, credentials)
├── Reviews & MoMs             (meeting notes, decisions)
├── Said — Weekly Updates      (active feature work — one sub-page per feature)
│    └── <Feature Name>        (current state, progress log, next actions)
└── AGENTS.md                  (this file's wiki mirror)
```

**Wiki page tokens (for lark-cli):**

| Page | obj_token | node_token |
|---|---|---|
| Said (root) | `NcivdaZVIopKLrxx1M9lDEMSg9e` | `SF2PwDlOliEnafkhrsrlfRd8gth` |
| Overview | `DXU5dllVBoXdmqxzIyrlWDFugAF` | `SOm1wO0lDiKztIkniQ2lzbhbgFf` |
| Architecture & Tech Stack | `XyE2dH0PVovQkRxNRoNliv3ggze` | `T0qUwaRwZiopOlkOLSzlTBSFgnt` |
| URLs & Access | `Kz2kdSj4to19vVxqbSGlx9HVgFf` | `DhHdwfy0aiBOoAkL68OlqwX0ggd` |
| Reviews & MoMs | `ZApkdhSbpo83jVxhVYWl6t5wgvc` | `Hh5Zw6GKgixy4vku4bKlCM6Gg6g` |
| Said — Weekly Updates (parent) | `I6TMdmYLForup1x6KTCl2KUngJq` | `E62IweE40i6z0WkvDBNloyVngwc` |
| Bug: Status Bar + Hang Fixes | `Z54Id6iGCoCJpwxT4Zcldez9gTg` | `VKZcwT45yinWx9kjZmmlqsQOgUh` |
| AGENTS.md | `FuKWd2RZDow8mcx3MvCl5wvygNc` | `RNFVwgB2biyXqbknUa2lShSDgkc` |

**Wiki space ID:** `7635896570625396443` (Tech Hub)

### How to read/write wiki pages

```bash
# Read a page
lark-cli docs +fetch --api-version v2 --doc <obj_token>

# Overwrite a page with new content
lark-cli docs +update --api-version v2 --doc <obj_token> --command overwrite --doc-format markdown --content @path/to/file.md

# Append to a page
lark-cli docs +update --api-version v2 --doc <obj_token> --command append --doc-format markdown --content "## New section\n\nContent here"

# Create a new sub-page under Weekly Updates
lark-cli wiki +node-create --space-id 7635896570625396443 --parent-node-token E62IweE40i6z0WkvDBNloyVngwc --title "Feature Name"
```

### At the START of every session

1. Fetch the **Said — Weekly Updates** page (`obj_token: I6TMdmYLForup1x6KTCl2KUngJq`) from the wiki
2. Read its current state — this is where the last AI left off
3. If the user's request doesn't match any active feature, ask before writing code

### During a session

- If you make an architectural decision that isn't obvious from the code, log it in the relevant Updates sub-page
- If you hit a blocker, note it in the Updates page immediately — don't leave it in local notes

### At the END of every session (before stopping)

1. **Overwrite** the Weekly Updates page (or the relevant feature sub-page) with a fresh snapshot of RIGHT NOW:
   - What is working
   - What is in progress (be specific: file + function level)
   - What is not started
   - The exact next action for whoever picks this up next
   - Append a progress log entry (date, tool name, what you did)
2. Push the updated content to the wiki using `lark-cli docs +update`

**This is not optional.** If you skip this step, the next AI session starts blind.
Treat updating the wiki as the last action you take in every session.

### When starting a brand-new feature

1. Create a sub-page under `Said — Weekly Updates` in the wiki
2. Fill in the current state and plan before writing any code

### Active Features

> Source of truth is the **Lark Wiki** under `Said — Weekly Updates`.
> Fetch the Updates page at session start to know where things stand.

| Feature | Status | Wiki obj_token |
|---|---|---|
| Bug: Status Bar + Hang Fixes | In progress — committed, needs push + test | `Z54Id6iGCoCJpwxT4Zcldez9gTg` |
