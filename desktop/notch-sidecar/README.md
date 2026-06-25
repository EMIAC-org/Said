# AirNote Notch HUD — native Swift sidecar

A standalone macOS executable that renders the AirNote status-bar pill as a
boring.notch-style **dynamic island** in/around the MacBook notch. It is an
*append-only* alternative to the Tauri-rendered status bar (`desktop/src/StatusBar.tsx`)
— the existing pill is untouched; this is enabled by an env flag.

```
Hold Caps Lock → amber dot + notch grows on the x-axis → live transcript rolls
              → polish → paste → transcript   ↘ learning + feedback cards
```

Listening is intentionally minimal: an amber recording dot in the notch bar and
**only the live transcript** in the chin (no label, no timer, no bars). The chin
expands to a max width then the transcript rolls inside it. The black notch
*shape* animates inside a fixed stage window — the window never resizes for the
voice flow, so the notch grows smoothly with no black "pop".

## Why native

The Tauri pill renders in a WebView driven by JS events. This sidecar is driven
by the **same Rust events**, forwarded in-process (no WebView hop) → lower
latency, and a true native notch shape that hugs the bezel / overlays the
physical camera cutout.

## How it connects (least-latency, append-only)

```
Rust (main.rs)  ──app.listen(...) taps──▶  stdin  ──▶  AirNoteNotch (Swift)
   status events (zero emit-site edits)              renders HUD state machine
                                          ◀── stdout ◀──  user actions
   handle_notch_action → backend HTTP                 confirm / retry / block …
```

- **Rust → Swift:** `desktop/src-tauri/src/notch_sidecar.rs` spawns the binary;
  `wire_notch_events()` in `main.rs` registers `app.listen` taps for every
  status event (`app-state`, `voice-level`, `voice-status`, `voice-*`,
  `vocab-*`, `auto-update-ready`, …) and forwards them as JSON lines.
- **Swift → Rust:** the sidecar writes action JSON lines on stdout;
  `handle_notch_action()` routes them to the same backend endpoints the React
  status bar uses (`/v1/confirm-term`, `confirm_batch`, `/v1/block-correction`,
  `retry_recording`).
- The sidecar **self-terminates** when stdin closes (Tauri exit) — no kill needed.

## Run it

```bash
# Build + stage the sidecar binary
just notch-build

# Run the full app with the notch HUD instead of the Tauri pill
AIRNOTE_NOTCH_SIDECAR=1 just dev

# Watch the real native notch cycle through every state (no Tauri):
just notch-demo
```

Off by default: without `AIRNOTE_NOTCH_SIDECAR=1` nothing spawns and the
current status bar behaves exactly as before.

## Wire protocol (newline-delimited JSON, snake_case)

**Inbound** `{"type": ...}`: `state` (`kind`: idle|recording|processing),
`level` (`value`), `status` (`phase`,`transcript`), `transcript`, `done`,
`output` (`status`,`message`), `error` (`message`,`audio_id`,`auto_hide_ms`),
`learned`, `email_saved`, `queued`, `wrong_fixed`, `confirm`, `negative_confirm`,
`review` (`candidates[]`), `retraining`, `retrain_done`, `update_ready`,
`placement`, `recents`, `present`, `dismiss`.

**Outbound** `{"type": ...}`: `ready`, `confirm` (`decision`), `confirm_batch`
(`items[]`), `block`, `retry` (`audio_id`), `dismiss`, plus stubs `undo`,
`apply_update`, `snooze_update`, `copy_recent`, `reposition`.

## Source map

| File | Role |
|---|---|
| `main.swift` | `.accessory` NSApplication bootstrap |
| `NotchController.swift` | brain: panel + geometry + bridge + message→state + sizing |
| `NotchPanel.swift` | borderless non-activating always-on-top `NSPanel` |
| `NotchGeometry.swift` | notch detection (`safeAreaInsets`/`auxiliaryTop*Area`) + flat fallback |
| `NotchShape.swift` / `NotchView.swift` | the flaring notch outline + root view (shape sized to `model.box`, top-centred in the fixed window) |
| `Cards.swift` | confirm / negative / review / error / update / toast / recents |
| `HUDModel.swift` | `HUDState` enum (one case per BarState family) |
| `Bridge.swift` / `Protocol.swift` | stdio JSON-lines transport + codable types |

## Deferred (follow-ups)

- **Bundling:** not added to `tauri.conf.json` `externalBin` yet (that would
  force the binary for every build and break the Windows bundle). Ship via a
  macOS-specific config at cutover. Dev resolves via the `.build` path.
- **Actions:** `undo`, `apply_update`, `snooze_update`, `copy_recent`,
  `reposition` are emitted by the HUD but not yet routed in `handle_notch_action`.
- **Hover-open recents:** the `recents` state renders when Rust sends it; the
  idle-hover trigger + history feed is not wired.
- **Divo** agent panels are intentionally out of scope (separate surface).

Design reference: `boring.notch/` (cloned, gitignored). Interactive HTML mock:
`design-previews/notch-hud-preview.html`.
