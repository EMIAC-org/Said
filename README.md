<!-- <img width="282" height="100" alt="Screenshot 2026-05-06 at 7 46 34 AM" src="https://github.com/user-attachments/assets/1455e803-0170-4e4b-85c5-f73feeb65641" /> -->
<div align="center">

# Said

### Voice dictation for macOS that actually understands the way you speak.

Hold Caps Lock. Speak. Release. Said types polished text into any app —
in English, Hindi, Hinglish, or whatever mix comes out of your mouth.

[![License: MIT](https://img.shields.io/badge/license-MIT-black.svg?style=flat-square)](LICENSE)
[![Platform: macOS](https://img.shields.io/badge/platform-macOS-black.svg?style=flat-square)](#requirements)
[![Latest Release](https://img.shields.io/github/v/release/EMIAC-org/Said?style=flat-square&color=black)](https://github.com/EMIAC-org/Said/releases)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-black.svg?style=flat-square)](https://www.rust-lang.org/)

<br />

<!--
  HERO IMAGE — replace with the floating menu-bar / status-bar capture
  showing Said live (the small pill that appears while recording).
  Recommended: 720px wide, dark background, transparent PNG.
-->
<img src="https://github.com/user-attachments/assets/1455e803-0170-4e4b-85c5-f73feeb65641" alt="Said running in the macOS menu bar" width="720" />

</div>

---

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/EMIAC-org/Said/main/install.sh | bash
```

That's it. The installer pulls the latest signed-free build, sets up a LaunchAgent, and leaves you with one command:

```bash
said              # start
said stop         # stop
said update       # pull latest
said status       # is it alive?
said logs         # tail logs
```

<details>
<summary>Other ways to install</summary>

- **From source:** `git clone` this repo, then `just dev` for the desktop app or `cargo build --release -p said` for the standalone CLI. See [Build from source](#build-from-source).
- **DMG:** grab `Said_<version>_aarch64.dmg` (Apple Silicon) or `Said_<version>_x86_64.dmg` (Intel) from the [latest release](https://github.com/EMIAC-org/Said/releases/latest).

</details>

---

## Why Said

<table>
<tr>
<td width="33%" valign="top">

### Hinglish, natively

Most dictation tools force you to pick one language and punish you for code-switching. Said treats Hinglish as a first-class output mode — preserves the Hindi-English mix the way you actually speak it, never silently outputs Devanagari, never "auto-corrects" `bhai` into `boy`.

A deterministic post-LLM romanizer guarantees the script you asked for, no matter what the model does.

</td>
<td width="33%" valign="top">

### Whisper-Flow speed, on the free tier

Time-to-first-token measured at **~150–400 ms** through Groq's LPU hardware. Polished text streams into your focused field token-by-token while you're still letting go of the key.

Free means free: bring your own Deepgram key (generous free tier) and sign in with the ChatGPT account you already have. No subscription. No credit card.

</td>
<td width="33%" valign="top">

### Learns from every edit

Wispr Flow guesses your jargon. Said *remembers* it.

When you fix a word, Said classifies the edit (STT mistake vs. polish mistake vs. you-changed-your-mind), runs it through three hallucination gates, and promotes confirmed corrections into a personal vocabulary that biases the next transcription.

The same word lands right the second time.

</td>
</tr>
</table>

---

<div align="center">

<!--
  DASHBOARD IMAGE — replace with the desktop app screenshot.
  Recommended: 1200px wide, captures the Home / Dashboard / History view.
-->
<img src="docs/assets/dashboard.png" alt="Said desktop dashboard" width="1100" />

<sub>The desktop app. History, vocabulary, and learning insights live here — but you'll spend most of your time never opening it.</sub>

</div>

---

## How it stacks up

| | **Said** | Wispr Flow | VoiceInk | SuperWhisper |
|---|:---:|:---:|:---:|:---:|
| Open source | Yes | No | Yes | No |
| Free tier | Yes (BYO keys) | Limited words/week | Yes | Limited |
| Hinglish / code-switching | First-class | Partial | English-only by default | Partial |
| Learns from your edits | Yes | Partial | No | No |
| Streams tokens as it polishes | Yes | Yes | No | No |
| Local-only mode | Roadmap | No | Yes | Yes |
| Time-to-first-token | ~300 ms | ~400 ms | Local-bound | Local-bound |

Receipts in code, not marketing — see [`crates/backend/src/llm/script.rs`](crates/backend/src/llm/script.rs) for the Hinglish guarantees, [`crates/backend/src/llm/groq.rs`](crates/backend/src/llm/groq.rs) for the speed path, and [`crates/backend/src/llm/promotion_gate.rs`](crates/backend/src/llm/promotion_gate.rs) for the learning gates.

---

## How it works

```
   Caps Lock           Deepgram nova-3              Groq / Codex
   ─────────           ────────────────             ────────────
   hold to record  ─►  streamed STT     ─►  polish (LLM, streaming)
                                                    │
                                                    ▼
                                              type into focused field
                                                    │
                                                    ▼
                                          watch for your edits (30s)
                                                    │
                                                    ▼
                                           classify  →  validate  →  learn
```

Five components, all in this repo:

- [**`crates/hotkey`**](crates/hotkey) — global Caps Lock listener (CGEventTap), hold-to-talk or push-to-toggle.
- [**`crates/recorder`**](crates/recorder) — CoreAudio capture, streamed straight to STT.
- [**`crates/core`**](crates/core) — Deepgram WebSocket client, gateway routing, shared types.
- [**`crates/backend`**](crates/backend) — local Axum daemon: SQLite history, vocabulary, embeddings, the learning pipeline, prefs.
- [**`crates/paster`**](crates/paster) — Accessibility-API typing into the focused field, with edit-watch.
- [**`desktop/`**](desktop) — Tauri shell, React UI, menu-bar tray.

A standalone `said` CLI binary ([`crates/said`](crates/said)) wires the above together for headless use without the desktop app.

---

## Quick start

After install:

1. **Sign in to OpenAI** (uses your existing ChatGPT account, no API key needed):
   ```bash
   said auth
   ```
2. **Add a Deepgram key** (free tier covers normal use):
   ```bash
   said deepgram-key
   ```
3. **Grant the three macOS permissions** Said opens for you:
   ```bash
   said permissions
   ```
   Microphone, Accessibility, Input Monitoring. Said will not phone home; everything except the LLM call lives on your machine.

Now hold Caps Lock anywhere on your Mac and speak.

---

## Requirements

- macOS 13 (Ventura) or later — Apple Silicon or Intel
- A Deepgram account (free tier is plenty) **or** the env vars to route through your own gateway
- A ChatGPT account, **or** any of: Groq API key, Gemini API key, OpenAI API key

See [`.env.example`](.env.example) for the full list of optional configuration.

---

## Build from source

```bash
git clone https://github.com/EMIAC-org/Said.git
cd Said
just dev          # builds the daemon, syncs the sidecar, launches Tauri
```

Common tasks (`just --list` for everything):

| | |
|---|---|
| `just dev` | run desktop app in dev mode |
| `just check` | fmt + clippy + test |
| `just dmg` | build a release DMG for the host arch |
| `cargo build -p said --release` | standalone CLI only |

Toolchain is pinned in [`rust-toolchain.toml`](rust-toolchain.toml) (stable, edition 2024). Node 20 for the desktop frontend.

---

## Configuration

Most settings are surfaced in the desktop app under **Settings**. The interesting ones:

- **Output language** — `english`, `hindi`, `hinglish`, `auto`. `hinglish` is the default.
- **Polish provider** — Codex (free, via your ChatGPT account), Groq (fastest), Gemini direct, OpenAI direct.
- **Tone preset** — neutral, professional, casual, assertive, concise, or a custom prompt.
- **Hotkey** — Caps Lock hold or toggle, with optional alternates.
- **Vocabulary** — review what Said has learned, edit terms, force-promote a word.

For headless / CLI users, the same settings live in `~/Library/Application Support/Said/`.

---

## Roadmap

- Local-only mode (whisper.cpp + on-device polish) for full offline use
- Linux support (waiting on a clean equivalent of the macOS Accessibility paste path)
- More languages with first-class code-switch support (Tamil-English, Tagalog-English, Spanglish)
- Team vocabulary sync via the optional self-hostable control plane

---

## Contributing

PRs welcome. The codebase is split for hackability — most contributions touch one of: `crates/backend/src/llm/` (the learning pipeline), `crates/hotkey` (input handling), `desktop/src` (UI), or `landing/` (marketing site).

Before opening a PR:

```bash
just check        # fmt + clippy + tests
```

Architecture notes and design rationale live in [`docs/`](docs). The learning pipeline in particular is documented end-to-end in [`docs/permission-learning.md`](docs/permission-learning.md).

---

## License

[MIT](LICENSE).
