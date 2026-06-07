# AirNote iOS

This is the native iOS implementation surface for AirNote Mobile.

## Targets To Create

- `AirNoteApp`: SwiftUI main app.
- `AirNoteKeyboard`: UIKit custom keyboard extension.
- `AirNoteShared`: shared models, App Group bridge, gateway client, event queue, and secure storage helpers.

## Current State

The source skeleton is in place for Phase 0:

- App Group bridge models and atomic file helper.
- Main app session controller and SwiftUI shell with readiness, setup, recovery, and live-session surfaces.
- Keyboard state machine and UIKit keyboard controller skeleton with voice bar, waveform/status states, manual QWERTY fallback, result preview, insert/copy/save acknowledgements, and stale-result suppression.
- Shared gateway/event/vocab/recovery models.

The `.xcodeproj` should be generated after the senior-dev iOS plan confirms bundle IDs, signing team, minimum iOS version, and whether the first spike is app-owned mic only or includes a direct keyboard-extension audio experiment.

## Verification Available Before Xcode Project Generation

- `swiftc -parse-as-library -emit-module -module-name AirNoteShared mobile/ios/AirNoteShared/*.swift`
- `cargo test -p airnote-mobile-core`
- JSON/JSONL fixture parse for `mobile/shared/schemas` and `mobile/shared/fixtures`

Full app and keyboard builds require full Xcode with the iOS SDK plus generated app, extension, test, and signing targets.

## Bundle Plan

```text
Main app:  com.emiac.airnote.ios
Keyboard:  com.emiac.airnote.ios.keyboard
App Group: group.com.emiac.airnote
```

## Desktop Guard

The iOS app must not depend on, launch, or modify the desktop Tauri shell, local Axum backend, HID paster, hotkey, recorder, or updater path.
