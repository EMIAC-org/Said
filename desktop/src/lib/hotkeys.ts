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
