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
5. Verify the AirNote bubble appears and can be dragged.

## Live Backend Later

Mock UI should pass first. For live testing, add the Android gateway client and use the Mac LAN IP or hosted staging gateway. Do not route Android through the desktop app or desktop local backend.

