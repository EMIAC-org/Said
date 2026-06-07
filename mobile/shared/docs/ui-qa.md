# iOS UI QA Gate

No Internal TestFlight build until these pass on real devices.

## Screens

- Welcome
- Account
- Privacy
- Language and style
- Vocabulary seed
- Microphone permission
- Keyboard setup
- Full Access repair
- Practice dictation
- Home
- Live session
- History
- Vocabulary
- Settings
- Diagnostics

## Keyboard States

- manual typing, no Full Access
- setup incomplete
- session stale
- ready
- recording
- processing STT
- processing polish
- insert preview
- inserted
- copied
- saved to history
- network retry
- permission repair
- unsupported secure field
- keyboard reloaded after result

## Acceptance

- Text fits on small iPhone, standard iPhone, and Pro Max.
- Controls are thumb safe.
- VoiceOver labels exist for every keyboard control.
- Dynamic Type does not overlap critical controls.
- Reduced Motion still communicates state.
- No spinner-only failure state.
- Every failure offers retry, copy, save, repair, or cancel.
