# AirNote Android

Native Android implementation surface for AirNote Mobile.

## Current State

- Jetpack Compose app scaffold under `app/`.
- Desktop-aligned dark UI tokens matching the iOS mock-mode app.
- Setup flow from first launch through:
  - account preview,
  - privacy defaults,
  - microphone check,
  - Android floating bubble setup,
  - bubble plus existing-keyboard preview.
- Accessibility service stub for the AirNote floating bubble.
- Unit tests for setup ordering and preview states.

## Android-Specific Product Rule

Android keeps the user's existing keyboard. AirNote appears as a floating bubble above that keyboard when a text field is focused.

The bubble is backed by an Accessibility Service so it can observe focused text fields and later insert/copy dictated text. The current implementation is still mock-mode UI and setup scaffolding; live dictation/backend wiring remains a later wave.

## Build

Install Android command-line tools or Android Studio, then from this folder:

```bash
./gradlew :app:assembleDebug
./gradlew :app:testDebugUnitTest
```

If using command-line tools only, install:

```bash
sdkmanager "platform-tools" "platforms;android-36" "build-tools;36.0.0"
```

## Desktop Guard

The Android app must not depend on, launch, or modify the desktop Tauri shell, local Axum backend, HID paster, hotkey, recorder, or updater path.

