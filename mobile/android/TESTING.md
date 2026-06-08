# AirNote Android Testing Guide

## UI Review Without Backend

1. Open `mobile/android` in Android Studio, or use the command line.
2. Build the debug app:

```bash
./gradlew :app:assembleDebug
```

3. Install on emulator or Android phone:

```bash
adb install -r app/build/outputs/apk/debug/app-debug.apk
```

4. Run AirNote and tap through:

```text
Start setup -> Use account -> accept privacy defaults -> Run mic check -> Bubble preview enabled -> Preview bubble -> Finish setup
```

5. Confirm the UI matches iOS:

- near-black background;
- compact dark cards;
- restrained periwinkle accent;
- white primary action buttons;
- same setup rhythm;
- no marketing hero layout.

6. Confirm the Android difference:

- setup talks about Floating Bubble, not iOS Keyboard;
- preview shows an AirNote Bubble above the existing keyboard;
- final dashboard can replay setup.

## Floating Bubble Service Check

1. Install the debug APK.
2. Open Android Settings -> Accessibility.
3. Enable `AirNote Bubble`.
4. Return to any app with a text field.
5. Verify the AirNote bubble appears and can be dragged by the `|||` handle.

## Live Gateway Smoke

Use a release or staging build. Do not route Android through the desktop app or desktop local backend.

Prerequisites:

- Gateway account exists.
- Account has active Deepgram and Groq runtime credentials in the server vault.
- Android device has network access to `https://airnote.emiactech.com`.

Run:

1. Install the release APK:

```bash
./gradlew :app:assembleRelease
adb install -r app/build/outputs/apk/release/app-release-unsigned.apk
```

2. Open AirNote, sign in during setup, accept privacy defaults, and finish setup.
3. From the dashboard, tap `Open voice session`, speak a short Hinglish or English phrase, tap `Stop and polish`, and confirm Recent shows the polished output.
4. Enable `AirNote Bubble` in Android Accessibility settings.
5. Open Notes, Messages, or another editable text field.
6. Tap the bubble, speak, tap `Stop`, then tap `Insert`.
7. Confirm the polished text appears at the cursor.
8. Test a secure field such as a password input. The bubble must show copy behavior, not text insertion.
9. In the control-plane admin/runtime view or database, confirm a `runtime_runs` entry and server history item exist for the Android `client_run_id`.

Expected fallback behavior:

- If the app is not signed in, the bubble opens the main app.
- If microphone permission is missing, the bubble opens the main app for recovery.
- If Android cannot paste into the target field, AirNote copies the polished text instead of replacing existing non-empty field content.
