# AirNote Android

Native Android implementation surface for AirNote Mobile.

## Current State

- Jetpack Compose app scaffold under `app/`.
- Desktop-aligned dark UI tokens matching the iOS app.
- Setup flow from first launch through:
  - Gateway account sign-in in live builds,
  - privacy defaults,
  - microphone check,
  - Android floating bubble setup,
  - bubble plus existing-keyboard preview.
- Gateway client for auth, runtime status/settings, and `POST /v1/runtime/voice/wav`.
- KeyStore-backed session storage for the Gateway bearer token and account.
- Dashboard and accessibility bubble can record 16 kHz WAV audio, send it to the server runtime, and expose insert/copy recovery.
- Unit tests for setup ordering, preview states, runtime labels, and voice phases.

## Android-Specific Product Rule

Android keeps the user's existing keyboard. AirNote appears as a floating bubble above that keyboard when a text field is focused.

The bubble is backed by an Accessibility Service so it can observe focused text fields and insert/copy dictated text. It records after an explicit tap, uploads to the mobile Gateway, pastes into normal editable fields, and copies only for secure or unsupported fields.

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
