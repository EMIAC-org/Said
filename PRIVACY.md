# Privacy Policy

**Last updated: 2026-07-26**

Said is a voice-to-text dictation app. This document describes what data leaves your machine, where it goes, and what stays local. It is written plainly so you can audit it against the source code.

If anything here is unclear or appears to contradict the code, please open an issue at https://github.com/EMIAC-org/Said/issues — the source is the truth.

---

## TL;DR

| Data | Where it goes | Why |
|---|---|---|
| Microphone audio (while a hotkey is held) | **This device only** for local STT; **DeepInfra or ElevenLabs** when that cloud route is selected | Speech-to-text |
| Transcript text | **Groq** servers | LLM polish |
| Optional correction context (snippets of edits you make) | **Gemini** API (Google) | Embedding for the on-device learning lexicon |
| Anonymous crash reports + lifecycle events | **Sentry** servers | Diagnose crashes and regressions |
| Polished text, transcripts, corrections, lexicon, audio (1-day retention) | **Your local SQLite database** | All app state lives on your machine |

We never sell or share your data. We send the minimum required to make the app work and to detect crashes.

---

## What we send to which third party

Said orchestrates local and configured external services to convert your speech into polished text.

### 1. Speech-to-text
- **Local route**: Nothing is sent to a speech provider. Raw audio is transcribed on your device.
- **Cloud routes**: The completed push-to-talk recording is sent directly to the selected provider, either DeepInfra Whisper or ElevenLabs Scribe v2.
- **What is received**: A transcript.
- **When**: Only while you are dictating. Audio capture stops on key-release.
- **API key**: None for local STT. Cloud-route credentials are configured by the build.

### 2. Groq / configured text polish runtime
- **What is sent**: The local transcript text, plus a polish prompt and any keyterms / replacements you've added to your local lexicon.
- **What is received**: Polished, streamed tokens that get typed into your focused application.
- **When**: Immediately after a transcript chunk arrives during dictation.
- **Retention on their side**: Per Groq's policy.
- **API key**: Yours, configured in Settings.

### 3. Gemini (embeddings, optional)
- **What is sent**: Small text snippets (a correction phrase you've made to dictated text) to compute a 256-dim embedding for the local lexicon.
- **What is received**: A vector of 256 floats, stored locally in SQLite.
- **When**: After you edit dictated text and Said's 30-second edit watch classifies the change as a learnable correction.
- **Optional**: If `GEMINI_API_KEY` is not configured, learning quality degrades but Said still works.

### 4. Sentry (diagnostics, opt-out)
- **What is sent**: App version, OS, architecture, an anonymous device UUID (generated locally, never tied to your identity), and any panic stacktrace or error-level log line that occurs.
- **What is NOT sent**: transcripts, polished text, audio, API keys, user-home file paths (we redact `/Users/...` to `~/...`), correction text, or anything you have dictated.
- **When**: On crash, on startup, on update check.
- **Default**: On. You can disable it in Settings → Diagnostics. Toggling it off stops sends within ~30 seconds.

---

## What stays local

All app state — preferences, your lexicon, correction history, edit-watch records, recent transcripts, and short-retention audio snippets — lives in a local SQLite database at:

- **macOS**: `~/Library/Application Support/VoicePolish/db.sqlite`
- **Windows** (planned): `%APPDATA%\Said\db.sqlite`

Audio files are retained on disk for **24 hours** for replay/debugging and then deleted by a background sweeper.

API keys you enter in Settings are stored in this SQLite database, unencrypted at rest. The database file inherits your user account's file permissions.

---

## Anonymous device ID

The first time Said launches, it generates a random UUID (e.g. `7b9d3e4f-...`). This UUID is stored locally in the preferences DB and is sent with Sentry events to deduplicate crashes from the same machine. It is not tied to your name, email, IP address, or any other identifier we control. Deleting the database resets it.

---

## Opt-out

- **Sentry**: Settings → Diagnostics → off. Toggle is honored within ~30 seconds; no events are sent while off.
- **Cloud speech providers**: On supported Macs, select the local speech route to keep microphone audio on-device. Cloud-locked devices require a configured cloud route.
- **Audio retention**: clear it manually with `said audio clear`, or delete the audio dir directly.

---

## Network endpoints

For audit purposes, the only outbound hosts Said contacts are:

- `api.groq.com` (LLM polish)
- `generativelanguage.googleapis.com` (Gemini embeddings, optional)
- `*.ingest.sentry.io` (diagnostics, opt-out)
- `github.com/EMIAC-org/Said/releases/...` (update manifest + downloads)

Block any of these at the firewall level if you prefer. Said degrades gracefully when a service is unreachable (no crash, no polish, just transcript-only output where applicable).

---

## Changes to this policy

Material changes will bump the **Last updated** date at the top and be announced in release notes. The full history is in `git log -- PRIVACY.md`.

---

## Contact

Bug or privacy concern: https://github.com/EMIAC-org/Said/issues
