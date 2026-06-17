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
- XcodeGen project spec for app, keyboard extension, and shared framework targets.
- App/keyboard plist and App Group entitlement files.
- Debug/Staging/Release gateway config files.

Generate the `.xcodeproj` with:

```bash
cd mobile/ios
xcodegen generate --spec project.yml
open AirNote.xcodeproj
```

The project spec is intentionally generated, not hand-written, so signing/settings stay reviewable.

## Verification Available Before Xcode Project Generation

- `swiftc -parse-as-library -emit-module -module-name AirNoteShared mobile/ios/AirNoteShared/*.swift`
- `cargo test -p airnote-mobile-core`
- JSON/JSONL fixture parse for `mobile/shared/schemas` and `mobile/shared/fixtures`

Full app and keyboard builds require full Xcode with the iOS SDK plus generated app, extension, test, and signing targets.

## iPhone Testing

See `TESTING.md`.

Use direct Xcode install first. Use the manual GitHub `iOS TestFlight` workflow only after direct device archive/export is working.

## Bundle Plan

```text
Main app:  com.emiac.airnote.ios
Keyboard:  com.emiac.airnote.ios.keyboard
App Group: group.com.emiac.airnote
```

## Desktop Guard

The iOS app must not depend on, launch, or modify the desktop Tauri shell, local Axum backend, HID paster, hotkey, recorder, or updater path.
