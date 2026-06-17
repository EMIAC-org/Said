# AirNote iPhone Testing Guide

This is the simple path after the coding part is ready.

## What You Need

1. A Mac with full Xcode installed.
2. An iPhone connected to the Mac.
3. Apple Developer access for team `96ZQGP7L3B`.
4. These Apple Developer IDs created:
   - App ID: `com.emiac.airnote.ios`
   - Keyboard ID: `com.emiac.airnote.ios.keyboard`
   - App Group: `group.com.emiac.airnote`
5. App Group enabled on both the app and keyboard IDs.
6. XcodeGen installed.

Install XcodeGen:

```bash
brew install xcodegen
```

## First Test: Run Directly On Your iPhone

This is the fastest testing method. Use this before TestFlight.

1. Open Terminal at repo root.
2. Generate the Xcode project:

```bash
cd mobile/ios
xcodegen generate --spec project.yml
open AirNote.xcodeproj
```

3. In Xcode, select the `AirNote` scheme.
4. Select your connected iPhone.
5. Confirm signing uses team `96ZQGP7L3B`.
6. Press Run.

## Turn On The Keyboard On iPhone

After the app installs:

1. Open iPhone Settings.
2. Go to General -> Keyboard -> Keyboards.
3. Add `AirNote Keyboard`.
4. Tap `AirNote Keyboard`.
5. Turn on `Allow Full Access`.
6. Open Notes or Messages and switch to AirNote Keyboard.

## Testing With The Gateway

Release and Staging builds should use the hosted mobile Gateway:

```text
https://airnote.emiactech.com
```

Debug builds can stay in mock mode for UI review. If you need a local Gateway from an iPhone, the iPhone cannot use `localhost` to reach your Mac. It needs your Mac LAN IP.

Example:

```text
http://192.168.1.10:3100
```

Change this file before generating/running the Xcode project:

```text
mobile/ios/Config/Debug.xcconfig
```

Important:

- Keep the `http:/$()/` shape in `.xcconfig` files. Xcode turns that into `http://`.
- The current Debug config still uses mock mode:

```text
AIRNOTE_USE_MOCK_GATEWAY = YES
```

For real LAN gateway testing, change it to:

```text
AIRNOTE_USE_MOCK_GATEWAY = NO
```

For production-like testing, use `Staging` or `Release`; both set:

```text
AIRNOTE_USE_MOCK_GATEWAY = NO
AIRNOTE_GATEWAY_BASE_URL = https:/$()/airnote.emiactech.com
```

## TestFlight Later

Use TestFlight after direct iPhone testing works.

Before TestFlight, GitHub needs these secrets:

```text
APP_STORE_CONNECT_API_KEY_ID
APP_STORE_CONNECT_API_ISSUER_ID
APP_STORE_CONNECT_API_PRIVATE_KEY
```

Also add final iOS App Icon assets before a real App Store/TestFlight upload.

Then run the manual GitHub workflow:

```text
iOS TestFlight
```

Start with:

```text
upload_to_testflight = false
```

That only creates an IPA artifact. Once archive/export works, run it again with:

```text
upload_to_testflight = true
```

## What To Test First

1. App opens.
2. Keyboard appears in iPhone Settings.
3. Full Access can be enabled.
4. AirNote Keyboard opens in Notes.
5. Next keyboard globe button works.
6. Manual keys type.
7. Live setup requires Gateway sign-in in Staging/Release.
8. App and keyboard share App Group session files.
9. Start a live AirNote Session from the keyboard, speak once, and confirm the server sends `runtime.done`.
10. The polished result can be inserted once.
11. The same result cannot insert twice.
12. Secure or unsupported fields use copy/recovery instead of unsafe insertion.
13. Control-plane runtime/history shows the iOS run.
