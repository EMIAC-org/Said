<div align="center">

<img src="https://github.com/user-attachments/assets/6c89ecc1-1f30-4f85-ad6e-7ed35e4c8f1e" alt="Said" width="760" />

<br />

### *Voice dictation for macOS that actually understands the way you speak.*

Hold Caps Lock. Speak. Release. Said types polished text into any app —
in English, Hindi, Hinglish, or whatever mix comes out of your mouth.

<br />

[![License MIT](https://img.shields.io/badge/license-MIT-5BD55B?labelColor=0B0B0B&style=flat)](LICENSE)
[![macOS only](https://img.shields.io/badge/macOS-13%2B-5BD55B?labelColor=0B0B0B&style=flat)](#requirements)
[![TTFT](https://img.shields.io/badge/TTFT-150–400ms-5BD55B?labelColor=0B0B0B&style=flat)](#why-said)
[![Hinglish-native](https://img.shields.io/badge/Hinglish-native-5BD55B?labelColor=0B0B0B&style=flat)](#why-said)
[![Latest release](https://img.shields.io/github/v/release/EMIAC-org/Said?color=5BD55B&labelColor=0B0B0B&style=flat)](https://github.com/EMIAC-org/Said/releases)
[![Built with Rust](https://img.shields.io/badge/Rust-edition%202024-5BD55B?labelColor=0B0B0B&style=flat)](https://www.rust-lang.org/)

<sub>

[Install](#install) &nbsp;·&nbsp; [Why Said](#why-said) &nbsp;·&nbsp; [How it works](#how-it-works) &nbsp;·&nbsp; [Quick start](#quick-start) &nbsp;·&nbsp; [Build](#build-from-source) &nbsp;·&nbsp; [Roadmap](#roadmap)

</sub>

</div>

---

## Install

### macOS

```bash
curl -fsSL https://raw.githubusercontent.com/EMIAC-org/Said/main/install.sh | bash
```

That's it. The installer pulls the latest build, registers a LaunchAgent, and leaves you with one command:

```bash
said              # start
said stop         # stop
said update       # pull latest
said status       # is it alive?
said logs         # tail logs
```

Apple Silicon and Intel both work; minimum macOS 13.

> [!NOTE]
> Said is **ad-hoc signed** (no paid Apple Developer ID). The first time you open the DMG, macOS shows "Said cannot be opened because the developer cannot be verified." Right-click the app → **Open** → confirm. After that it launches normally.

### Windows (beta)

1. Download `Said_<version>_x64-setup.exe` from the [latest release](https://github.com/EMIAC-org/Said/releases/latest).
2. Run the installer.

> [!IMPORTANT]
> Said is **not Authenticode-signed** for the v3.0 beta. Windows SmartScreen will block the installer with **"Windows protected your PC"**. Click **More info** → **Run anyway** to proceed. Code signing is on the roadmap; the binary contents are the same builds the macOS users run.

After install:

- **Hold Caps Lock** to dictate. Release to type polished text into the focused window. The Caps Lock toggle never fires — Said suppresses the OS-level toggle while the app is running (so you don't accidentally end up in ALL CAPS).
- Caps Lock not your preference? **Settings → Hold key → Right Alt**.
- No permission prompts on Windows — `WH_KEYBOARD_LL` and `SendInput` work for non-elevated apps without setup.

Windows known limitations in v3.0:

- No Authenticode signing — SmartScreen warning on first run (see above).
- The **30-second edit-watch** that learns from your corrections falls back to clipboard-only on Windows (the UIAutomation tree-read port is a follow-up). Hinglish polish, polish prompts, and the tone shortcuts work fully.
- Windows ARM (Surface) is not in v3.0; x64 only.

<details>
<summary>Other ways to install</summary>

- **From source:** `git clone` this repo, then `just dev` for the desktop app or `cargo build --release -p said` for the standalone CLI (macOS only for now). See [Build from source](#build-from-source).
- **macOS DMG:** grab `Said_<version>_aarch64.dmg` (Apple Silicon) or `Said_<version>_x86_64.dmg` (Intel) from the [latest release](https://github.com/EMIAC-org/Said/releases/latest).
- **Windows installer:** `Said_<version>_x64-setup.exe` on the same release page.

</details>

---

## Why Said

<table>
<tr>
<td width="33%" valign="top">

### Hinglish, natively

Most dictation tools force you to pick a language and punish you for code-switching. Said treats Hinglish as a first-class output mode — preserves the Hindi-English mix the way you actually speak, never silently outputs Devanagari, never "auto-corrects" `bhai` into `boy`.

A hand-written 80-glyph Devanagari→Roman romanizer ([`script.rs`](crates/backend/src/llm/script.rs)) runs as a deterministic post-LLM guard. The model can drift; the script can't.

</td>
<td width="33%" valign="top">

### Wispr-Flow speed, on the free tier

**~150–400 ms time-to-first-token** measured through Groq's LPU hardware ([`groq.rs:4`](crates/backend/src/llm/groq.rs)). Polished text streams into your focused field token-by-token while you're still letting go of the key.

Local whisper.cpp speech recognition runs on-device first; the backend only receives transcript text for polishing.

Free means free: install the local speech model and sign in with the ChatGPT account you already have for polish.

</td>
<td width="33%" valign="top">

### Learns from every edit

Wispr Flow guesses your jargon. Said *remembers* it.

When you fix a word, a 4-way classifier on Groq llama-3.1-8b-instant (~150 ms) labels the edit as `STT_ERROR`, `POLISH_ERROR`, `USER_REPHRASE` or `USER_REWRITE`. Three hallucination gates ([`promotion_gate.rs`](crates/backend/src/llm/promotion_gate.rs)) verify it. Confirmed corrections land in a 256-d embedding store and bias the next transcription.

Same word lands right the second time.

</td>
</tr>
</table>

---

## See it speak Hinglish

```diff
- haan toh basically meeting reschedule karni hai bhai kal subah ke liye
+ Haan, toh basically meeting reschedule karni hai bhai — kal subah ke liye.
```

```diff
- mereko lagta hai ki ye vali approach better hai because faster hai aur cheaper bhi
+ Mereko lagta hai ki ye wali approach better hai — because faster hai, aur cheaper bhi.
```

And when the model has a bad day and emits Devanagari, the post-LLM romanizer pulls it back:

```text
model emits:   आज बहुत काम था, मैं थक गया हूँ
Said outputs:  Aaj bahut kaam tha, main thak gaya hoon.
```

Always Roman, never Devanagari — guaranteed by a deterministic 80-glyph romanizer that runs *after* every polish ([`script.rs:104`](crates/backend/src/llm/script.rs)).

---

<div align="center">

<img src="https://github.com/user-attachments/assets/16e6564c-c752-4515-b3df-8db9d49ddca5" alt="Said desktop dashboard" width="1100" />

<sub>The desktop app. History, vocabulary, and learning insights live here — but you'll spend most of your time never opening it.</sub>

</div>

---

## How it stacks up

<div align="center">

| | **Said** | Wispr Flow | VoiceInk | SuperWhisper |
|---|:---:|:---:|:---:|:---:|
| Open source | **Yes** | No | Yes | No |
| Free tier | **Yes (BYO keys)** | Limited words/week | Yes | Limited |
| Hinglish / code-switching | **First-class** | Partial | English-only by default | Partial |
| Learns from your edits | **Yes** | Partial | No | No |
| Streams tokens as it polishes | **Yes** | Yes | No | No |
| Local-only mode | Roadmap | No | Yes | Yes |

</div>

Receipts in code, not marketing — see [`script.rs`](crates/backend/src/llm/script.rs) for the Hinglish guarantees, [`groq.rs`](crates/backend/src/llm/groq.rs) for the speed path, [`classifier.rs`](crates/backend/src/llm/classifier.rs) for the 4-way edit classifier, [`promotion_gate.rs`](crates/backend/src/llm/promotion_gate.rs) for the three hallucination gates.

---

## How it works

```
   Caps Lock           local whisper.cpp           Groq / Codex
   ─────────           ─────────────────           ────────────
   hold to record  ──► local transcript ──► polish (LLM, streaming)
                                                    │
                                                    ▼
                                              type into focused field
                                                    │
                                                    ▼
                                          watch for your edits (30s)
                                                    │
                                                    ▼
                                      classify ─► validate ─► learn
                                       (4-way)    (3 gates)   (256-d
                                                              embed)
```

> "**The transcript is NOT ground truth.** STT errors are exactly the case where transcript and polish agree on the wrong word."
> &nbsp;
> — [`crates/backend/src/llm/classifier.rs:20`](crates/backend/src/llm/classifier.rs)

Six components, all in this repo:

- [**`crates/hotkey`**](crates/hotkey) — global Caps Lock listener (CGEventTap), hold-to-talk or push-to-toggle.
- [**`crates/recorder`**](crates/recorder) — CoreAudio/WASAPI capture at 16 kHz.
- [**`crates/core`**](crates/core) — shared speech transcript metadata and polish helpers.
- [**`crates/backend`**](crates/backend) — local Axum daemon. SQLite (20 migrations), 7 vocabulary-related tables, 256-d embeddings, the learning pipeline, prefs.
- [**`crates/paster`**](crates/paster) — Accessibility-API typing into the focused field, with edit-watch.
- [**`desktop/`**](desktop) — Tauri shell, React UI, menu-bar tray, 39 commands.

A standalone `said` CLI binary ([`crates/said`](crates/said)) wires the above together for headless use without the desktop app.

---

## Quick start

After install:

1. **Sign in to OpenAI** (uses your existing ChatGPT account, no API key needed):
   ```bash
   said auth
   ```
2. **Install or verify the local speech model** in onboarding/settings.
3. **Grant the three macOS permissions** Said opens for you:
   ```bash
   said permissions
   ```
   Microphone, Accessibility, Input Monitoring. Said never phones home; everything except the LLM call lives on your machine.

Now hold Caps Lock anywhere on your Mac and speak.

<details>
<summary>Why Caps Lock?</summary>

Three reasons:

1. **It's the largest unused key on a Mac.** Easy to find by feel.
2. **Hold-to-talk doesn't conflict with shortcuts.** Anything that starts with ⌘, ⌥, ⌃ stays free.
3. **The toggle behavior is suppressed when held > 200 ms.** Tap it and it still toggles caps; hold it and it dictates. You don't lose the key.

If you'd rather use a different key, switch the hotkey under Settings or in [`crates/hotkey/src/lib.rs`](crates/hotkey/src/lib.rs).

</details>

---

## Requirements

- macOS 13 (Ventura) or later — Apple Silicon or Intel
- The local speech model installed by onboarding/settings
- A ChatGPT account, **or** any configured text-polish gateway/provider key

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

Toolchain pinned in [`rust-toolchain.toml`](rust-toolchain.toml) — stable Rust, edition 2024. Node 20 for the desktop frontend.

---

## Configuration

Most settings live in the desktop app under **Settings**. The interesting ones:

- **Output language** — `english`, `hindi`, `hinglish`, `auto`. `hinglish` is the default.
- **Polish provider** — Codex (free, via your ChatGPT account), Groq (fastest), Gemini direct, OpenAI direct.
- **Tone preset** — neutral, professional, casual, assertive, concise, or a custom prompt.
- **Hotkey** — Caps Lock hold or toggle, with optional alternates.
- **Vocabulary** — review what Said has learned, edit terms, force-promote a word.

For headless / CLI users, the same settings live in `~/Library/Application Support/Said/`.

---

## What's inside

A few numbers to set expectations:

| | |
|---|---|
| Total LOC (Rust + TS) | ~43,000 |
| Crates | 7 |
| DB migrations | 20 |
| Tauri commands | 39 |
| Vocabulary-related tables | 7 |
| End-to-end learning tests | 15 |
| Embedding dimension | 256 |
| Devanagari glyph map | 80 |

---

## Roadmap

- **Local-only mode** — whisper.cpp + on-device polish for full offline use
- **Linux support** — waiting on a clean equivalent of the macOS Accessibility paste path
- **More code-switched languages** — first-class Tamil-English, Tagalog-English, Spanglish
- **Team vocabulary sync** — via the optional self-hostable control plane

---

## Contributing

PRs welcome. The codebase is split for hackability — most contributions touch one of:

- [`crates/backend/src/llm/`](crates/backend/src/llm) — the learning pipeline
- [`crates/hotkey`](crates/hotkey) — input handling
- [`desktop/src`](desktop/src) — UI
- [`landing/`](landing) — marketing site

Before opening a PR:

```bash
just check        # fmt + clippy + tests
```

Architecture notes and design rationale live in [`docs/`](docs).

---

## License

[MIT](LICENSE).

<div align="center">
<br />
<sub>

Built in obsidian and mint by [@anish877](https://github.com/anish877). &nbsp;·&nbsp; The transcript is not ground truth.

</sub>
</div>
