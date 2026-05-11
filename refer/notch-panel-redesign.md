# Said — Expanded Notch Panel Redesign

## Current layout (640x190)

```
┌─────────────────────────────────────────────────────────────┐
│                        ┌──────┐                             │
│                        │NOTCH │                             │
│  Said                  └──────┘                        (x)  │
│                                                             │
│  Hold your hotkey and speak. Polished text pastes           │
│  automatically.                                             │
│                                                             │
│  ┌─────────────────────────────────────────────────────┐    │
│  │          🎙  Start Recording                        │    │
│  └─────────────────────────────────────────────────────┘    │
│                                                             │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐           ⚙       │
│  │ Hinglish │ │ English  │ │  Hindi   │                     │
│  └──────────┘ └──────────┘ └──────────┘                     │
└─────────────────────────────────────────────────────────────┘
```

**Problems:**
- Big record button wastes space — hotkey is the primary input, not clicking
- Static helper text adds no value after first use
- Language pills are passive — no checkmark showing which is active
- Gear icon does nothing useful (no settings panel yet)
- No live info — last result, shortcut hints, status
- Doesn't feel like a quick-glance dashboard

---

## Proposed layout (640x190)

```
┌─────────────────────────────────────────────────────────────┐
│                       ┌──────┐                              │
│                       │NOTCH │                              │
│  Said                 └──────┘                         (x)  │
│                                                             │
│ ┌───────────────────────────┐  ┌──────────────────────────┐ │
│ │                           │  │                          │ │
│ │  LAST RESULT              │  │  QUICK POLISH  select    │ │
│ │                           │  │  text first, then:       │ │
│ │  "Mujhe samajh nahi aa    │  │                          │ │
│ │   raha ki kya chal raha   │  │  Professional      ⌥2   │ │
│ │   hai. 203 times I..."    │  │  Casual             ⌥3   │ │
│ │                           │  │  Concise            ⌥4   │ │
│ │            ⌘⌃V to repaste │  │  Hinglish           ⌥5   │ │
│ │                           │  │                          │ │
│ └───────────────────────────┘  └──────────────────────────┘ │
│                                                             │
│  ● Ready    ⇪ Caps Lock     Hinglish ▾     🎙    ⚙  ⟳    │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

### Anatomy — 3 rows

#### Row 1: Title bar

```
┌─────────────────────────────────────────────────────┐
│  Said                                          (x)  │
└─────────────────────────────────────────────────────┘
```

- App name left, close button right
- Starts below the hardware notch (`padding(.top, notchHeight)`)

#### Row 2: Two-column content (main area)

```
┌──────────────────────┐  ┌──────────────────────┐
│   LEFT: Last Result  │  │  RIGHT: Quick Polish  │
│                      │  │                       │
│  3-line preview of   │  │  4 tone buttons       │
│  last polished text  │  │  with ⌥ shortcuts     │
│                      │  │                       │
│  ⌘⌃V to repaste     │  │  "select text first"  │
└──────────────────────┘  └──────────────────────┘
```

**Left panel — Last Result**
- Shows last polished text (3 lines max, truncated)
- If nothing yet: "Hold ⇪ and speak" placeholder
- Bottom-right: `⌘⌃V` repaste hint (faded)
- Tapping copies to clipboard

**Right panel — Quick Polish**
- 4 tone buttons: Professional, Casual, Concise, Hinglish
- Each shows ⌥ shortcut on the right
- Small hint at top: "select text first, then:"
- Clicking a button triggers the polish (same as ⌥2-5)

#### Row 3: Status bar (bottom)

```
┌───────────────────────────────────────────────────────┐
│  ● Ready    ⇪ Caps Lock     Hinglish ▾    🎙  ⚙  ⟳  │
└───────────────────────────────────────────────────────┘
```

| Element | Purpose |
|---|---|
| `● Ready` | Live status (green=ready, red=recording, yellow=processing) |
| `⇪ Caps Lock` | Shows which hotkey is active |
| `Hinglish ▾` | Current output language, tap to cycle |
| `🎙` | Manual record button (compact icon, not a big CTA) |
| `⚙` | Opens Settings/Dashboard window |
| `⟳` | Re-paste last result (`⌘⌃V`) |

---

## State variations

### When recording (hotkey held)

Panel stays closed — notch chin shows audio bars (compact state).
If panel is already open, status bar updates:

```
│  🔴 Listening...    ⇪ Caps Lock     Hinglish      🎙  ⚙     │
```

### When nothing polished yet (first launch)

Left panel shows onboarding hint:

```
┌──────────────────────┐
│                      │
│    Hold ⇪ and speak  │
│                      │
│    Polished text     │
│    pastes into any   │
│    app automatically │
│                      │
└──────────────────────┘
```

### When last result has error

Left panel shows error:

```
┌──────────────────────┐
│                      │
│  ✗ API key not       │
│    accepted.         │
│    Check Settings.   │
│                      │
└──────────────────────┘
```

---

## Comparison

| Aspect | Current | Proposed |
|---|---|---|
| Last result | Not shown | 3-line preview + repaste hint |
| Polish shortcuts | Not in panel | 4 buttons with ⌥ hints |
| Record button | Giant CTA (wastes space) | Small icon in status bar |
| Status | None | Live dot + text |
| Hotkey hint | Static helper text | Shows actual key in status bar |
| Language | 3 passive pills | Compact toggle showing current |
| Settings | Dead gear icon | Opens main window |
| Information density | Low — mostly empty space | High — every pixel useful |
