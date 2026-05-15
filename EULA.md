# End-User License Agreement

**Last updated: 2026-05-15**

Said is free, open-source software released under the MIT License. This document is a plain-English summary of the terms you accept by using Said. The legally binding terms are in the [LICENSE](LICENSE) file at the root of this repository.

---

## What you can do

- Run Said on any computer you control, for any purpose (personal, commercial, internal business, research).
- Read, modify, fork, redistribute the source code.
- Bundle Said into another product, with or without modification.

The MIT license at [LICENSE](LICENSE) is the authoritative grant.

---

## What we promise

- We will not silently exfiltrate your data. See [PRIVACY.md](PRIVACY.md) for the exact list of services Said calls and what is sent to each.
- The auto-updater only ships releases signed with the project's update key. The public key is checked into the source tree at `desktop/src-tauri/tauri.conf.json`.
- Releases are versioned semantically. Breaking changes warrant a major version bump.

---

## What we don't promise

Said is provided **"as is"** without warranty of any kind. The full disclaimer is in [LICENSE](LICENSE), but in plain terms:

- **No SLA**: Said is a desktop app you install yourself. Crashes, missed words, transcription errors, network failures, third-party API outages, and platform breakage are all possible and not grounds for refund or compensation (there is nothing to refund — Said is free).
- **No data recovery**: if your local SQLite database is corrupted or deleted, your lexicon and history are gone. Back up `~/Library/Application Support/VoicePolish/db.sqlite` if it matters to you.
- **Third-party services**: Deepgram, Groq, and Gemini have their own terms and outages. We do not control them. If they break, Said breaks.

---

## Your responsibilities

- **API keys you enter** are yours; you pay any usage costs to the underlying providers. Said does not proxy any LLM/STT traffic — your keys talk directly to the providers.
- **Accessibility permissions you grant** are scoped to Said only. Said uses them to type polished text into your focused application; it does not read your screen, monitor keystrokes outside of the configured hotkey, or upload anything related to other applications.
- **Caps Lock interception (Windows)**: if you enable Caps Lock as the dictation hotkey on Windows, Said suppresses the Caps Lock toggle while running. You can switch to a different hotkey in Settings if this conflicts with your workflow.

---

## Trademarks

"Said" and the Said logo are used in this repository under the MIT license; you may use them when redistributing unmodified copies of the source. Modified forks should use a different name to avoid confusion.

---

## Updates to this EULA

Material changes will bump the **Last updated** date at the top and be announced in release notes. Continuing to use Said after a change constitutes acceptance.

If you do not accept the terms, uninstall Said and stop using it.
