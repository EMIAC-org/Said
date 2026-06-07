# App Store Review Notes Draft

AirNote iOS uses a custom keyboard extension and a main-app recording session.

## Review Explanation

- AirNote Keyboard provides keyboard input functionality and a next-keyboard path.
- Voice dictation requires Full Access because the keyboard communicates with the AirNote app and hosted AirNote Mobile Gateway.
- The main app owns microphone permission and shows a visible user-initiated session before recording.
- AirNote records only after the user taps the mic or starts a session.
- AirNote does not run in secure password fields.
- Some apps and fields do not support third-party keyboards; AirNote saves the result to history or offers copy/retry instead.
- Provider keys are never shipped in the iOS app or keyboard extension.

## Demo Account

Create a staging demo account before TestFlight external review.

## Required Review Assets

- Screen recording of setup.
- Screen recording of AirNote Keyboard dictation into Notes.
- Screen recording of Full Access repair state.
- Privacy policy URL.
- Support URL.
