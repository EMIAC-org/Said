// Platform-aware key-name formatting.
//
// One source of truth so we never hardcode macOS glyphs (⌘ ⌥ ⌃ ⇧ ↵) on Windows.
// Pass a canonical hotkey token string (e.g. "ctrl", "ctrl+n", "cmd+enter",
// "cmd+shift+p", or a stored pref like "right_option" / "caps_lock") plus the
// platform, and get back the display label for that platform.

export type Platform = "macos" | "windows" | "linux";

// macOS renders glyphs and joins with no separator (⌘⇧P).
const MAC: Record<string, string> = {
  cmd: "⌘", command: "⌘", meta: "⌘", super: "⌘", mod: "⌘",
  ctrl: "⌃", control: "⌃",
  opt: "⌥", option: "⌥", alt: "⌥",
  shift: "⇧",
  enter: "↵", return: "↵",
  esc: "⎋", escape: "⎋", tab: "⇥", backspace: "⌫", delete: "⌦", space: "Space",
  right_option: "Right Option", caps_lock: "Caps Lock", fn: "Fn",
};

// Windows/Linux render words and join with "+". Cmd/Meta map to Ctrl because the
// app's chords use the Cmd key as the shortcut modifier (handlers accept
// metaKey || ctrlKey), so cmd+shift+p === Ctrl+Shift+P on Windows.
const WIN: Record<string, string> = {
  cmd: "Ctrl", command: "Ctrl", meta: "Ctrl", super: "Win", mod: "Ctrl",
  ctrl: "Ctrl", control: "Ctrl",
  opt: "Alt", option: "Alt", alt: "Alt",
  shift: "Shift",
  enter: "Enter", return: "Enter",
  esc: "Esc", escape: "Esc", tab: "Tab", backspace: "Backspace", delete: "Delete", space: "Space",
  right_option: "Right Alt", caps_lock: "Caps Lock", fn: "Caps Lock",
};

// Accept already-rendered glyphs as input too, so a stored value like "⌘⇧P" still
// normalizes correctly.
const GLYPH_TO_TOKEN: Record<string, string> = {
  "⌘": "cmd", "⌃": "ctrl", "⌥": "opt", "⇧": "shift",
  "↵": "enter", "⏎": "enter", "⎋": "esc", "⇥": "tab", "⌫": "backspace", "⌦": "delete",
};

function expandGlyphs(segment: string): string[] {
  // Split a glyph-run like "⌃N" or "⌘⇧P" into ["ctrl","n"] / ["cmd","shift","p"].
  if (![...segment].some((ch) => GLYPH_TO_TOKEN[ch])) return [segment];
  const out: string[] = [];
  let rest = "";
  for (const ch of segment) {
    if (GLYPH_TO_TOKEN[ch]) {
      if (rest) { out.push(rest); rest = ""; }
      out.push(GLYPH_TO_TOKEN[ch]);
    } else {
      rest += ch;
    }
  }
  if (rest) out.push(rest);
  return out;
}

function titleCase(token: string): string {
  return token
    .split("_")
    .map((w) => (w ? w[0].toUpperCase() + w.slice(1) : w))
    .join(" ");
}

// ── Hold-to-talk record hotkey ────────────────────────────────────────────────
//
// The set of keys that can be a "hold to record" trigger. Deliberately limited to
// sided modifiers + Caps Lock + Fn: a bare typing key can't be a global hold key
// (the OS hook would have to swallow it, so you could never type that letter).
// Each id maps to platform keycodes/masks/VKs in crates/hotkey (`RecordHotkey::from_id`).

export type HotkeyId =
  | "caps_lock"
  | "fn"
  | "left_command"
  | "right_command"
  | "left_control"
  | "right_control"
  | "left_option"
  | "right_option"
  | "left_shift"
  | "right_shift";

interface HotkeyDef {
  id: HotkeyId;
  macGlyph: string;
  macLabel: string;
  winGlyph: string;
  winLabel: string;
  macOnly?: boolean;
}

// Ordered "recommended first" — Caps Lock, then the popular right-hand modifiers.
const HOTKEY_DEFS: HotkeyDef[] = [
  { id: "caps_lock", macGlyph: "⇪", macLabel: "Caps Lock", winGlyph: "⇪", winLabel: "Caps Lock" },
  { id: "right_command", macGlyph: "⌘", macLabel: "Right ⌘", winGlyph: "⊞", winLabel: "Right Win" },
  { id: "right_control", macGlyph: "⌃", macLabel: "Right ⌃", winGlyph: "Ctrl", winLabel: "Right Ctrl" },
  { id: "right_option", macGlyph: "⌥", macLabel: "Right ⌥", winGlyph: "Alt", winLabel: "Right Alt" },
  { id: "fn", macGlyph: "fn", macLabel: "Fn / Globe", winGlyph: "", winLabel: "", macOnly: true },
  { id: "left_command", macGlyph: "⌘", macLabel: "Left ⌘", winGlyph: "⊞", winLabel: "Left Win" },
  { id: "left_control", macGlyph: "⌃", macLabel: "Left ⌃", winGlyph: "Ctrl", winLabel: "Left Ctrl" },
  { id: "left_option", macGlyph: "⌥", macLabel: "Left ⌥", winGlyph: "Alt", winLabel: "Left Alt" },
  { id: "right_shift", macGlyph: "⇧", macLabel: "Right ⇧", winGlyph: "⇧", winLabel: "Right Shift" },
  { id: "left_shift", macGlyph: "⇧", macLabel: "Left ⇧", winGlyph: "⇧", winLabel: "Left Shift" },
];

export interface HotkeyOption {
  id: HotkeyId;
  glyph: string;
  label: string;
}

/** The pickable hold-to-talk keys for a platform (Fn is macOS-only). */
export function hotkeyOptions(platform: Platform): HotkeyOption[] {
  const win = platform === "windows";
  return HOTKEY_DEFS.filter((d) => !(d.macOnly && win)).map((d) => ({
    id: d.id,
    glyph: win ? d.winGlyph : d.macGlyph,
    label: win ? d.winLabel : d.macLabel,
  }));
}

/** Display (glyph + label) for a stored hotkey id. Unknown ids and Windows-Fn
 *  both fall back to Caps Lock (the backend degrades Fn on Windows). */
export function hotkeyDisplay(id: string, platform: Platform): HotkeyOption {
  const win = platform === "windows";
  const d = HOTKEY_DEFS.find((x) => x.id === id);
  if (!d || (d.macOnly && win)) {
    return { id: "caps_lock", glyph: "⇪", label: "Caps Lock" };
  }
  return { id: d.id, glyph: win ? d.winGlyph : d.macGlyph, label: win ? d.winLabel : d.macLabel };
}

/** Map a DOM KeyboardEvent.code to a supported hold-to-talk id, or null if the
 *  key can't be a hold trigger (e.g. a letter — would break typing that key). */
export function codeToHotkeyId(code: string): HotkeyId | null {
  switch (code) {
    case "MetaLeft":
      return "left_command";
    case "MetaRight":
      return "right_command";
    case "ControlLeft":
      return "left_control";
    case "ControlRight":
      return "right_control";
    case "AltLeft":
      return "left_option";
    case "AltRight":
      return "right_option";
    case "ShiftLeft":
      return "left_shift";
    case "ShiftRight":
      return "right_shift";
    case "CapsLock":
      return "caps_lock";
    default:
      return null;
  }
}

/** Runtime behaviour of a hotkey (mirrors crates/hotkey): macOS Caps Lock is a
 *  locking key → tap to start / tap to stop; everything else is hold-to-talk. */
export function hotkeyMode(id: string, platform: Platform): "hold" | "toggle" {
  return id === "caps_lock" && platform !== "windows" ? "toggle" : "hold";
}

// ── Conflict / consequence warnings ───────────────────────────────────────────
//
// Our trigger is a single held key, so combo-collision tables (VoiceTypr's
// hotkey-conflicts.ts) don't apply — but a few single keys carry real
// consequences the user should know before committing. `error` = likely to
// misbehave; `warning` = works, but has a side effect worth flagging.

export interface HotkeyWarning {
  severity: "error" | "warning";
  text: string;
}

/**
 * Return a consequence/conflict warning for a chosen hold-to-talk key, or null
 * if it's a clean choice. Platform-aware — the same key means different things
 * on macOS vs Windows.
 */
export function hotkeyWarning(id: string, platform: Platform): HotkeyWarning | null {
  const win = platform === "windows";
  switch (id) {
    case "caps_lock":
      // We repurpose Caps Lock, so caps-typing is effectively disabled while
      // it's the trigger. On macOS it also toggles rather than holds.
      return {
        severity: "warning",
        text: win
          ? "While Caps Lock is your trigger, you can’t use it to type in ALL CAPS."
          : "Caps Lock taps to start/stop (not hold), and typing in ALL CAPS is disabled while it’s your trigger.",
      };
    case "fn":
      // macOS-only; Fn/Globe is the system Dictation / emoji key.
      return {
        severity: "warning",
        text: "Fn / Globe also opens macOS Dictation or the emoji picker — if it double-fires, pick a modifier instead.",
      };
    case "right_command":
      return win
        ? {
            severity: "warning",
            text: "The right Windows key opens the Start menu — some apps may steal it.",
          }
        : {
            severity: "warning",
            text: "Some apps bind Right ⌘ — if it doesn’t fire in one, try a different modifier.",
          };
    case "left_command":
      return win
        ? { severity: "warning", text: "The left Windows key opens the Start menu." }
        : null;
    case "left_option":
    case "left_control":
    case "left_shift":
      // Left-hand modifiers are the ones you naturally press while typing
      // shortcuts, so a hold-to-talk on them fires during normal chords.
      return {
        severity: "warning",
        text: "Left-hand modifiers fire during normal keyboard shortcuts — a right-hand key is usually calmer.",
      };
    default:
      return null;
  }
}

/** Format a canonical hotkey string for the given platform. */
export function formatKeycap(hotkey: string, platform: Platform): string {
  const mac = platform === "macos";
  const table = mac ? MAC : WIN;
  const parts = hotkey
    .split("+")
    .flatMap((s) => expandGlyphs(s.trim()))
    .map((s) => s.trim().toLowerCase())
    .filter(Boolean);
  const rendered = parts.map((p) => table[p] ?? (p.length === 1 ? p.toUpperCase() : titleCase(p)));
  return rendered.join(mac ? "" : "+");
}
