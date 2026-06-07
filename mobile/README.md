# AirNote Mobile

This folder contains the mobile-only AirNote implementation surface.

Current priority:

- iOS first.
- Native SwiftUI main app plus UIKit keyboard extension.
- Hosted AirNote Mobile Gateway for STT, polish, vocabulary, learning, quota, and observability.
- Desktop app behavior must remain untouched.

## Working Rules

- Work only on the `anugra` branch until mobile is tested and approved.
- Keep mobile client code under `mobile/`.
- Keep deterministic shared helpers under `crates/mobile-core`.
- Do not move or modify desktop hotkey, recorder, paster, Tauri shell, updater, or sidecar behavior for mobile work.
- Use the shared schemas and fixtures before adding platform-specific behavior.

## Structure

```text
mobile/
  shared/
    schemas/     JSON contracts for bridge, voice, events, and vocab
    fixtures/    Golden payloads used by Rust and client tests
    docs/        Store review, privacy, UI QA, and setup notes
  ios/
    AirNoteApp/
    AirNoteKeyboard/
    AirNoteShared/
```

## Phase 0 Goals

1. Prove App Group bridge state can be shared between app and keyboard.
2. Prove duplicate/stale results cannot insert twice.
3. Prove the main app microphone session feasibility after the user returns to a host app.
4. Prove hosted gateway latency with real iPhone network.

The first implementation is intentionally contract-heavy. The UI and bridge should be validated before deep gateway or learning work begins.
