# Permission Learning From VoiceInk

This note captures how VoiceInk handles macOS permissions and adjacent capabilities, so we can reuse the good parts in Said while preserving Said's stronger auto-learning pipeline.

Source repo inspected: `/Users/abhishekverma/Desktop/Cluster/Projects/Misc/VoiceInk`

## Executive Summary

VoiceInk's permission strategy is simple and product-friendly:

1. Declare required usage strings in `Info.plist`.
2. Add matching entitlements for signed/local builds.
3. Ask for core permissions during onboarding, one at a time.
4. Keep a dedicated Permissions page that can re-check status and open the right System Settings pane.
5. At runtime, guard permission-sensitive features and degrade gracefully instead of crashing.
6. Re-check permission state when the app becomes active, because users often leave the app to grant permission in System Settings.

The user-facing permissions VoiceInk asks for are:

- Microphone
- Accessibility
- Screen Recording
- Keyboard shortcut setup
- Microphone device selection

It also uses supporting capabilities that are not shown as onboarding "permission cards":

- Apple Events automation, for browser URL detection and optional AppleScript paste
- Network client/server, for cloud transcription/enhancement providers
- User-selected file read-only, for audio/video import
- Keychain access groups and CloudKit in the signed build
- Screen capture entitlement
- Audio input entitlement

Important nuance: VoiceInk does not explicitly ask for an "Input Monitoring" permission in its permission UI. It uses `NSEvent.addGlobalMonitorForEvents` for modifier/middle-click hotkeys and the `KeyboardShortcuts` package for custom shortcuts. Our app currently has a more explicit Input Monitoring pathway for `CGEventTap`; if we keep Caps Lock/global event-tap behavior, we still need our Input Monitoring UX.

## Where Permissions Are Declared

### `Info.plist`

VoiceInk declares three TCC usage strings:

File: `/Users/abhishekverma/Desktop/Cluster/Projects/Misc/VoiceInk/VoiceInk/Info.plist`

- `NSMicrophoneUsageDescription`
  - "VoiceInk needs access to your microphone to record audio for transcription."
- `NSAppleEventsUsageDescription`
  - "VoiceInk needs to interact with your browser to detect the current website for applying website-specific configurations."
- `NSScreenCaptureUsageDescription`
  - "VoiceInk needs screen recording access to understand context from your screen for improved transcription accuracy."

Takeaway for Said:

- We should keep usage strings specific and feature-oriented.
- Add or confirm strings for microphone, accessibility-adjacent paste behavior, screen recording/context awareness, Apple Events/browser URL detection, and Input Monitoring if we keep CGEventTap.
- macOS does not use an `NSAccessibilityUsageDescription`; Accessibility permission is driven by the Accessibility API and System Settings pane, not a usage key.

### Entitlements

Production entitlements:

File: `/Users/abhishekverma/Desktop/Cluster/Projects/Misc/VoiceInk/VoiceInk/VoiceInk.entitlements`

Local-build entitlements:

File: `/Users/abhishekverma/Desktop/Cluster/Projects/Misc/VoiceInk/VoiceInk/VoiceInk.local.entitlements`

Common useful entitlements:

```xml
<key>com.apple.security.app-sandbox</key>
<false/>
<key>com.apple.security.automation.apple-events</key>
<true/>
<key>com.apple.security.device.audio-input</key>
<true/>
<key>com.apple.security.files.user-selected.read-only</key>
<true/>
<key>com.apple.security.network.client</key>
<true/>
<key>com.apple.security.network.server</key>
<true/>
<key>com.apple.security.screen-capture</key>
<true/>
```

Production-only extras:

- `keychain-access-groups`
- `com.apple.developer.icloud-container-identifiers`
- `com.apple.developer.icloud-services`
- `com.apple.developer.aps-environment`
- temporary mach lookup exceptions for Sparkle

Takeaway for Said:

- VoiceInk disables App Sandbox. That makes Accessibility, global paste, screen capture, Apple Events, local model files, and provider networking easier.
- If Said stays unsandboxed, permission behavior will be simpler.
- If Said uses App Sandbox later, we must revisit all automation/paste/screen-capture behaviors carefully.

## Permission UX Surfaces

### Main Permissions Page

File: `/Users/abhishekverma/Desktop/Cluster/Projects/Misc/VoiceInk/VoiceInk/Views/PermissionsView.swift`

VoiceInk has a `PermissionManager` with published state:

- `audioPermissionStatus`
- `isAccessibilityEnabled`
- `isScreenRecordingEnabled`
- `isKeyboardShortcutSet`

It checks all permissions in one method:

```swift
func checkAllPermissions() {
    checkAccessibilityPermissions()
    checkScreenRecordingPermission()
    checkAudioPermissionStatus()
    checkKeyboardShortcut()
}
```

It re-checks when the app becomes active:

```swift
NotificationCenter.default.addObserver(
    self,
    selector: #selector(applicationDidBecomeActive),
    name: NSApplication.didBecomeActiveNotification,
    object: nil
)
```

This is a strong pattern. After opening System Settings, users come back to the app; the UI refreshes automatically.

### Onboarding Permission Flow

File: `/Users/abhishekverma/Desktop/Cluster/Projects/Misc/VoiceInk/VoiceInk/Views/Onboarding/OnboardingPermissionsView.swift`

VoiceInk walks the user through permissions one at a time:

1. Microphone Access
2. Microphone Selection
3. Accessibility Access
4. Screen Recording
5. Keyboard Shortcut

The flow allows skipping most permissions for now. This is good UX because the user can reach the product without getting stuck, while missing features remain visible in settings.

### Metrics Setup

File: `/Users/abhishekverma/Desktop/Cluster/Projects/Misc/VoiceInk/VoiceInk/Views/Metrics/MetricsSetupView.swift`

VoiceInk repeats the core setup checklist in another entry point:

- Keyboard shortcut
- Accessibility
- Screen Recording
- Model download

Takeaway:

- Said should not bury permissions in one setup path only.
- The home/settings/dashboard views should all be able to show missing permission state and a direct action.

## Permission-by-Permission Details

## Microphone

VoiceInk checks:

```swift
AVCaptureDevice.authorizationStatus(for: .audio)
```

VoiceInk requests:

```swift
AVCaptureDevice.requestAccess(for: .audio) { granted in
    DispatchQueue.main.async {
        self.audioPermissionStatus = granted ? .authorized : .denied
    }
}
```

If already denied, it opens:

```swift
x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone
```

Files:

- `/Users/abhishekverma/Desktop/Cluster/Projects/Misc/VoiceInk/VoiceInk/Views/PermissionsView.swift`
- `/Users/abhishekverma/Desktop/Cluster/Projects/Misc/VoiceInk/VoiceInk/Views/Onboarding/OnboardingPermissionsView.swift`

Runtime behavior:

- VoiceInk's actual `VoiceInkEngine.requestRecordPermission` currently returns `true` directly. The permission prompt is handled earlier in onboarding/settings rather than at record time.
- Recording itself is done through CoreAudio/AVFoundation via `Recorder` and `CoreAudioRecorder`.

Takeaway for Said:

- We should not rely only on runtime recording failure to infer missing mic permission.
- Add an explicit mic permission check to the Tauri snapshot.
- If `.notDetermined`, show a "Request Permission" button.
- If `.denied`, open the Microphone privacy pane.
- If recording returns silence, keep the existing diagnostic, but that should be fallback, not the primary UX.

## Microphone Device Selection

VoiceInk treats input device selection as a setup step, not a macOS permission.

File: `/Users/abhishekverma/Desktop/Cluster/Projects/Misc/VoiceInk/VoiceInk/Services/AudioDeviceManager.swift`

It enumerates CoreAudio devices via:

- `kAudioHardwarePropertyDevices`
- `kAudioDevicePropertyStreamConfiguration`
- `kAudioDevicePropertyDeviceNameCFString`
- `kAudioDevicePropertyDeviceUID`

It supports:

- System default input
- Custom selected input
- Prioritized list of inputs
- Automatic fallback to built-in or first available input
- Device-change listener
- Mid-recording switch when a device disappears

Takeaway for Said:

- Add device selection as its own setup card, separate from permission.
- Store device UID, not just device ID.
- Prefer built-in mic on first run if available.
- Listen for device list changes and fall back gracefully.

## Accessibility

VoiceInk checks without prompting:

```swift
let options: NSDictionary = [
    kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: false
]
let enabled = AXIsProcessTrustedWithOptions(options)
```

VoiceInk requests during onboarding:

```swift
let options: NSDictionary = [
    kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: true
]
AXIsProcessTrustedWithOptions(options)
```

VoiceInk opens System Settings:

```swift
x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility
```

VoiceInk polls after requesting:

```swift
Timer.scheduledTimer(withTimeInterval: 0.5, repeats: true) { timer in
    if AXIsProcessTrusted() {
        timer.invalidate()
        permissionStates[currentPermissionIndex] = true
    }
}
```

Runtime uses:

- `CursorPaster.pasteFromClipboard` refuses to post Cmd+V unless `AXIsProcessTrusted()` is true.
- `CursorPaster.performAutoSend` refuses to post Enter/Shift-Enter/Cmd-Enter unless trusted.
- `AIEnhancementService.getSystemMessage` only fetches selected text when Accessibility is trusted.

Files:

- `/Users/abhishekverma/Desktop/Cluster/Projects/Misc/VoiceInk/VoiceInk/Views/PermissionsView.swift`
- `/Users/abhishekverma/Desktop/Cluster/Projects/Misc/VoiceInk/VoiceInk/Views/Onboarding/OnboardingPermissionsView.swift`
- `/Users/abhishekverma/Desktop/Cluster/Projects/Misc/VoiceInk/VoiceInk/CursorPaster.swift`
- `/Users/abhishekverma/Desktop/Cluster/Projects/Misc/VoiceInk/VoiceInk/Services/AIEnhancement/AIEnhancementService.swift`

Takeaway for Said:

- We already use Accessibility for paste and focused-text reads. Keep that.
- Improve UX by copying VoiceInk's pattern:
  - check with prompt false for status
  - request with prompt true only from a clear user action
  - open the Accessibility pane as fallback
  - poll every 500 ms after request/open
  - re-check on app activation
- Runtime should degrade:
  - If Accessibility is missing, still transcribe and copy to clipboard.
  - Disable auto-paste/edit-watcher instead of failing the whole recording.

## Screen Recording / Context Awareness

VoiceInk checks:

```swift
CGPreflightScreenCaptureAccess()
```

VoiceInk requests:

```swift
CGRequestScreenCaptureAccess()
```

VoiceInk opens System Settings:

```swift
x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture
```

VoiceInk polls after requesting exactly like Accessibility.

Runtime use:

File: `/Users/abhishekverma/Desktop/Cluster/Projects/Misc/VoiceInk/VoiceInk/Services/ScreenCaptureService.swift`

VoiceInk uses:

- `ScreenCaptureKit`
- `SCShareableContent.excludingDesktopWindows`
- active window selection by frontmost PID
- `SCScreenshotManager.captureImage`
- Vision OCR with `VNRecognizeTextRequest`
- `recognitionLevel = .accurate`
- `usesLanguageCorrection = true`
- `automaticallyDetectsLanguage = true`

It stores a tagged context section:

```text
Active Window: ...
Application: ...

Window Content:
...
```

Then `AIEnhancementService` injects it into the prompt as:

```text
<CURRENT_WINDOW_CONTEXT>
...
</CURRENT_WINDOW_CONTEXT>
```

Takeaway for Said:

- This is one of the highest-value VoiceInk features to port.
- Treat screen recording as optional "context awareness," not mandatory dictation.
- If missing, polish still works; it just lacks window context.
- Prompt copy should clearly say context improves transcription/polish accuracy and is not stored unless we explicitly decide to store it.
- We need a privacy boundary: capture window text just before/during recording, pass to polish request, do not persist by default.

## Keyboard Shortcuts

VoiceInk has two shortcut styles:

1. Custom shortcuts via the `KeyboardShortcuts` package.
2. Direct modifier-key and middle-click monitoring through `NSEvent.addGlobalMonitorForEvents`.

Files:

- `/Users/abhishekverma/Desktop/Cluster/Projects/Misc/VoiceInk/VoiceInk/HotkeyManager.swift`
- `/Users/abhishekverma/Desktop/Cluster/Projects/Misc/VoiceInk/VoiceInk/MiniRecorderShortcutManager.swift`
- `/Users/abhishekverma/Desktop/Cluster/Projects/Misc/VoiceInk/VoiceInk/PowerMode/PowerModeShortcutManager.swift`

It checks "permission" for keyboard shortcut by checking whether a shortcut is configured:

```swift
KeyboardShortcuts.getShortcut(for: .toggleMiniRecorder) != nil
```

It does not expose a separate Input Monitoring permission card.

Runtime behavior:

- Modifier keys are monitored via `.flagsChanged`.
- Middle-click is monitored via `.otherMouseDown` / `.otherMouseUp`.
- Custom shortcuts use `KeyboardShortcuts.onKeyDown` and `onKeyUp`.
- It has toggle, push-to-talk, and hybrid modes.

Takeaway for Said:

- VoiceInk's shortcut UX is clean: users configure intent, mode, and fallback shortcuts.
- But Said's Caps Lock / CGEventTap path is lower-level and should keep explicit Input Monitoring checks.
- We can copy the product shape, not the exact permission assumptions:
  - Show "Keyboard Shortcut" as a setup step.
  - Show "Input Monitoring" only when the chosen hotkey path needs CGEventTap.
  - If using a high-level shortcut package for alternate shortcuts, those can avoid some Input Monitoring complexity.

## Input Monitoring

No explicit VoiceInk request flow found.

VoiceInk does not appear to call:

- `IOHIDRequestAccess(kIOHIDRequestTypeListenEvent)`
- `IOHIDCheckAccess(kIOHIDRequestTypeListenEvent)`

Said currently does use a CGEventTap-style flow and has code/comments around Input Monitoring in:

- `/Users/abhishekverma/Desktop/Cluster/Projects/Misc/wispr-hindi-bridge/crates/hotkey/src/lib.rs`

Takeaway for Said:

- Do not remove Said's Input Monitoring pathway just because VoiceInk omits it.
- If we add VoiceInk-style custom shortcut support, we can make Input Monitoring conditional by hotkey type.
- For Caps Lock push-to-talk, global event taps probably still need explicit Input Monitoring handling.

## Apple Events / Browser URL Detection

VoiceInk declares:

```xml
<key>NSAppleEventsUsageDescription</key>
<string>VoiceInk needs to interact with your browser to detect the current website for applying website-specific configurations.</string>
```

It enables:

```xml
<key>com.apple.security.automation.apple-events</key>
<true/>
```

File: `/Users/abhishekverma/Desktop/Cluster/Projects/Misc/VoiceInk/VoiceInk/PowerMode/BrowserURLService.swift`

Runtime:

- Detects frontmost app.
- If it is a supported browser, runs a bundled `.scpt` file through `/usr/bin/osascript`.
- Extracts current URL.
- Uses URL to apply Power Mode configuration.

Supported browsers include:

- Safari
- Arc
- Chrome
- Edge
- Firefox
- Brave
- Opera
- Vivaldi
- Orion
- Zen
- Yandex

Takeaway for Said:

- If we copy app/browser-specific modes, Apple Events permission is needed for browser URL detection.
- Use browser URL only for routing prompt/model/context profile, not for storing sensitive history by default.
- Errors should be quiet: if URL detection fails, fall back to app bundle ID, then default config.

## Clipboard

VoiceInk reads and writes the general pasteboard.

Files:

- `/Users/abhishekverma/Desktop/Cluster/Projects/Misc/VoiceInk/VoiceInk/CursorPaster.swift`
- `/Users/abhishekverma/Desktop/Cluster/Projects/Misc/VoiceInk/VoiceInk/Services/AIEnhancement/AIEnhancementService.swift`

Clipboard behavior:

- Snapshot current clipboard items.
- Set transcription text.
- Post paste command.
- Restore old clipboard contents after a delay.
- Optionally use clipboard context in prompt.

No explicit macOS TCC prompt is needed for normal app pasteboard access, but user trust/privacy matters.

Takeaway for Said:

- We should keep clipboard restore.
- If we add clipboard context, make it an explicit toggle.
- Do not persist clipboard context.
- Add a size cap and redact obvious secrets if possible.

## Selected Text

VoiceInk fetches selected text for context if Accessibility is trusted.

File: `/Users/abhishekverma/Desktop/Cluster/Projects/Misc/VoiceInk/VoiceInk/Services/SelectedTextService.swift`

It uses `SelectedTextKit` with strategies:

```swift
let strategies: [TextStrategy] = [.accessibility, .menuAction]
```

Then `AIEnhancementService` injects it as:

```text
<CURRENTLY_SELECTED_TEXT>
...
</CURRENTLY_SELECTED_TEXT>
```

Takeaway for Said:

- Add selected-text context as a permission-gated optional context source.
- Use it before screen OCR because selected text is cleaner and less privacy-invasive than OCR.

## File Import

VoiceInk declares document types for audio/video files in `Info.plist` and uses:

```xml
<key>com.apple.security.files.user-selected.read-only</key>
<true/>
```

Supported extensions include:

- wav
- mp3
- m4a
- aiff
- mp4
- mov
- aac
- flac
- caf

Takeaway for Said:

- If we add audio-file transcription, use user-selected read-only entitlement and a file picker.
- Keep this separate from live dictation permissions.

## Network

VoiceInk enables:

```xml
<key>com.apple.security.network.client</key>
<true/>
<key>com.apple.security.network.server</key>
<true/>
```

Network is needed for:

- Cloud transcription providers
- AI enhancement providers
- Model/API verification
- Sparkle updates
- Local server/Ollama/local CLI scenarios

Takeaway for Said:

- Already needed for Local speech, Gemini, Groq, OpenAI/Gateway, app auth, and local backend.
- Keep permission/capability docs clear if we package with sandbox constraints later.

## Keychain / CloudKit / Notifications

VoiceInk production entitlements include:

- `keychain-access-groups`
- iCloud container/services
- push notification environment

These are not part of first-run permission UX.

Takeaway for Said:

- Keychain should be used for API keys/tokens if not already.
- Do not mix these into the visible permission checklist unless the user must act.

## Runtime Guard Patterns Worth Copying

### Guard Before Use

Examples:

- `CursorPaster.pasteFromClipboard` checks `AXIsProcessTrusted()` before posting Cmd+V.
- `AIEnhancementService.captureScreenContext` checks `CGPreflightScreenCaptureAccess()` before OCR.
- `AIEnhancementService.getSystemMessage` only reads selected text if Accessibility is trusted.

Said should apply this rule everywhere:

- Missing Accessibility: copy to clipboard, skip auto-paste and edit-watcher.
- Missing Screen Recording: skip OCR context.
- Missing Apple Events: skip browser URL, use app name only.
- Missing mic: block recording with direct CTA.

### Poll After Request

VoiceInk polls every 500 ms after Accessibility or Screen Recording request until granted.

This improves perceived reliability. Users do not have to manually refresh.

### Re-check On App Activation

VoiceInk re-checks permissions when the app becomes active. We should add the same pattern in Tauri:

- App window focus/app activation should trigger snapshot refresh.
- Frontend should also keep a modest polling fallback.

### Open Exact System Settings Pane

VoiceInk uses:

```text
x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone
x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility
x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture
```

For Said we should add:

```text
x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent
```

for Input Monitoring, if supported on the macOS versions we target. If that pane URL is unreliable, fall back to Security & Privacy and show copy telling the user where to click.

## Proposed Permission Model For Said

### Required For Core Dictation

- Microphone
- Network, if using cloud STT/LLM

Without these, dictation cannot run.

### Required For Seamless Paste + Learning

- Accessibility
- Input Monitoring, if using Caps Lock/global event tap

Without Accessibility, we can still return text, but cannot reliably paste/read focused text/capture edits.

### Optional Quality Boosters

- Screen Recording for OCR context
- Apple Events for browser URL detection
- Selected text/clipboard context toggles

Without these, dictation still works, just with less context.

### Optional Product Features

- User-selected file read-only for audio import
- Notifications for learned-word toasts
- Keychain for API key storage

## Suggested Said Permission Checklist

Onboarding should show:

1. Microphone Access
   - Required.
   - Request via native Tauri/macOS command.
2. Keyboard Shortcut
   - Required for hotkey workflow.
   - Explain selected mode: Caps Lock / Option / custom.
3. Input Monitoring
   - Required only for Caps Lock/global event tap.
   - Request/open settings.
4. Accessibility Access
   - Required for auto-paste and edit learning.
   - Still allow "copy only" mode if skipped.
5. Context Awareness
   - Optional.
   - Enables selected text, browser URL, clipboard, screen OCR.
6. Screen Recording
   - Optional sub-step if context awareness with OCR is enabled.

## What Not To Copy Blindly

- VoiceInk's `VoiceInkEngine.requestRecordPermission` simply returns `true`; do not copy that into Said. It works because their UI asks earlier, but it is not a robust runtime check.
- VoiceInk does not explicitly handle Input Monitoring; Said needs it for our current hotkey architecture.
- VoiceInk disables App Sandbox. If Said's release process differs, entitlements and automation behavior must be retested.
- VoiceInk stores screen context in memory for prompt use. If we add context capture, do not persist it casually.

## Porting Priority

1. Add a native permission manager in Tauri mirroring VoiceInk's `PermissionManager`.
2. Add `mic_granted`, `screen_recording_granted`, `input_monitoring_granted`, `apple_events_available`, and `keyboard_shortcut_configured` to `AppSnapshot`.
3. Add exact System Settings open commands for each permission.
4. Add onboarding/setup cards with skip support for optional permissions.
5. Add runtime guard degradation:
   - no AX => copy only
   - no screen recording => no OCR context
   - no Apple Events => no browser URL
6. Add context pack only after permission state is clean.

## Minimal Swift/macOS API Cheat Sheet

Microphone:

```swift
AVCaptureDevice.authorizationStatus(for: .audio)
AVCaptureDevice.requestAccess(for: .audio) { granted in ... }
```

Accessibility:

```swift
AXIsProcessTrusted()
AXIsProcessTrustedWithOptions([
    kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: true
] as NSDictionary)
```

Screen Recording:

```swift
CGPreflightScreenCaptureAccess()
CGRequestScreenCaptureAccess()
```

Open panes:

```swift
NSWorkspace.shared.open(URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")!)
NSWorkspace.shared.open(URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone")!)
NSWorkspace.shared.open(URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")!)
```

Screen OCR:

```swift
let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
let filter = SCContentFilter(desktopIndependentWindow: window)
let image = try await SCScreenshotManager.captureImage(contentFilter: filter, configuration: configuration)
let request = VNRecognizeTextRequest()
request.recognitionLevel = .accurate
request.usesLanguageCorrection = true
request.automaticallyDetectsLanguage = true
```

Accessibility-gated paste:

```swift
guard AXIsProcessTrusted() else { return }
CGEvent(keyboardEventSource: source, virtualKey: 0x09, keyDown: true)?.post(tap: .cghidEventTap)
```

## Bottom Line

VoiceInk's permission flow is not technically exotic. Its strength is that it makes permissions understandable and recoverable:

- It asks in the right order.
- It has a permanent Permissions page.
- It opens the exact System Settings pane.
- It polls and re-checks after users return.
- It treats context permissions as optional quality boosters.

For Said, the best version is VoiceInk's permission UX plus our stricter runtime requirements for auto-paste, edit capture, and auto-learning.
