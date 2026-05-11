# Said — Tray Menu Redesign

## Current (bad)

```
┌──────────────────────────────┐
│ Start Recording              │
│──────────────────────────────│
│ Output Language           ▸  │ ← submenu: Hinglish/English/Hindi
│ Polish my message         ▸  │ ← submenu: 5 tone options
│──────────────────────────────│
│ Open Said                    │
│ Settings…              ⌘,   │ ← does same thing as Open Said
│──────────────────────────────│
│ Quit Said              ⌘Q   │
└──────────────────────────────┘
```

**Problems:**
- No live status — user can't tell if Said is listening/connected
- "Start Recording" feels out of place as top item (hotkey is primary)
- Two items do the same thing (Open Said / Settings)
- Language selection buried in submenu, no current selection visible
- Polish shortcuts buried, no keyboard shortcut hints in parent
- No hotkey reminder — new users forget what key to hold
- Flat waveform icon never changes — no visual feedback

---

## Proposed Design

### Menu bar icon states

```
Idle:        waveform      (static, dim)
Recording:   waveform      (pulsing/highlighted)  
Processing:  waveform      (spinning or animated)
Error:       waveform      (red dot badge)
```

### Menu structure

```
┌──────────────────────────────────┐
│                                  │
│  ● Ready                         │  ← green dot = backend connected
│  Hold ⇪ Caps Lock to dictate    │  ← shows active hotkey
│                                  │
│──────────────────────────────────│
│                                  │
│  QUICK POLISH            ⌥2–5   │  ← section header
│                                  │
│  ✦ Professional           ⌥2    │
│  ✦ Casual                 ⌥3    │
│  ✦ Concise                ⌥4    │
│  ✦ Hinglish               ⌥5    │
│                                  │
│──────────────────────────────────│
│                                  │
│  Language: Hinglish        ▸     │  ← shows current, submenu to switch
│     ┌────────────────────┐       │
│     │ ✓ Hinglish         │       │
│     │   English          │       │
│     │   Hindi            │       │
│     └────────────────────┘       │
│                                  │
│──────────────────────────────────│
│                                  │
│  Dashboard…              ⌘D     │  ← opens main window
│  Settings…               ⌘,     │  ← opens main window → settings tab
│                                  │
│──────────────────────────────────│
│                                  │
│  Quit Said               ⌘Q     │
│                                  │
└──────────────────────────────────┘
```

### State changes

```
                 ┌─────────┐
                 │  Idle   │
                 │ ● Ready │
                 └────┬────┘
                      │ user holds hotkey
                      ▼
               ┌──────────────┐
               │  Recording   │
               │ 🔴 Listening │  ← icon pulses, status text changes
               └──────┬───────┘
                      │ user releases hotkey
                      ▼
              ┌───────────────┐
              │  Processing   │
              │ ◐ Polishing…  │  ← icon animates
              └───────┬───────┘
                      │
            ┌─────────┴─────────┐
            ▼                   ▼
    ┌──────────────┐    ┌──────────────┐
    │    Done      │    │    Error     │
    │ ✓ Pasted     │    │ ✗ API error  │
    └──────┬───────┘    └──────┬───────┘
           │ 2s                │ 3s
           └─────────┬─────────┘
                     ▼
               ┌──────────┐
               │   Idle   │
               │ ● Ready  │
               └──────────┘
```

### Key differences from current

| Aspect | Current | Proposed |
|---|---|---|
| Status | None | Live ● Ready / 🔴 Listening / ✓ Pasted |
| Hotkey hint | None | "Hold ⇪ Caps Lock to dictate" |
| Polish shortcuts | Buried in submenu | Top-level with ⌥ hints |
| Language | Buried, no current shown | Shows current inline |
| Redundancy | Open Said + Settings do same thing | Dashboard + Settings (separate) |
| Icon | Static | Animates per state |
| Smart Repair ⌥1 | Listed alongside tones | Removed — it's unimplemented |

### Implementation notes

- Use `@ObservedObject` on the engine's `notchVM` to reactively update status text and icon
- Menu bar icon: SF Symbol `waveform` with `.symbolEffect(.pulse)` during recording
- Status header is a non-interactive `Text` view (not a button)
- Polish items call `engine.handleShortcutPublic(n)` directly
- Language submenu reads current from backend prefs, shows checkmark on active
