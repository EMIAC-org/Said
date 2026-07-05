import { useCallback, useEffect, useState } from "react";

/** The two concrete palettes the app can render. */
export type Theme = "dark" | "light";

/** What the user actually *chose*. `"system"` follows the OS appearance live;
 *  `"dark"` / `"light"` pin a palette regardless of the OS. */
export type ThemePreference = "system" | "dark" | "light";

const STORAGE_KEY = "vp-theme";

/** Read the OS appearance. Works in both the macOS WKWebView and the Windows
 *  WebView2 runtime (both honour `prefers-color-scheme`). Falls back to dark
 *  when `matchMedia` is unavailable (very old runtime / SSR). */
export function systemTheme(): Theme {
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") {
    return "dark";
  }
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

/** The saved preference. Absent / unrecognised → `"system"` so a brand-new
 *  user (who never picked a theme) simply follows their OS. */
export function readPreference(): ThemePreference {
  try {
    const v = localStorage.getItem(STORAGE_KEY);
    if (v === "dark" || v === "light" || v === "system") return v;
  } catch { /* ignore */ }
  return "system";
}

/** Collapse a preference into the concrete palette to render. */
function resolve(pref: ThemePreference): Theme {
  return pref === "system" ? systemTheme() : pref;
}

/**
 * Theme controller. Splits the user's *preference* (`system | dark | light`)
 * from the *resolved* palette (`dark | light`) that actually paints.
 *
 * - When the preference is `"system"`, the resolved theme tracks the OS and
 *   updates live via the `prefers-color-scheme` media query — no reload needed
 *   when the user flips macOS/Windows appearance.
 * - The resolved theme is written to `document.documentElement.dataset.theme`;
 *   the *preference* is persisted to localStorage.
 * - The initial paint is bootstrapped by a small inline script in index.html
 *   (no-flash) that runs the same resolution before React mounts.
 */
export function useTheme(): {
  /** The palette currently painting. */
  theme:      Theme;
  /** The user's stored choice (drives the Appearance picker selection). */
  preference: ThemePreference;
  /** Pin a concrete palette (Dark / Warm Paper). */
  setTheme:      (t: Theme) => void;
  /** Set any preference, including `"system"` (Follow system). */
  setPreference: (p: ThemePreference) => void;
  /** Topbar Sun/Moon — flips the *resolved* palette into an explicit choice. */
  toggle: () => void;
} {
  const [preference, setPreferenceState] = useState<ThemePreference>(readPreference);
  const [theme, setResolved] = useState<Theme>(() => resolve(readPreference()));

  // Apply + persist whenever the preference changes.
  useEffect(() => {
    const applied = resolve(preference);
    setResolved(applied);
    if (typeof document !== "undefined") {
      document.documentElement.dataset.theme = applied;
    }
    try { localStorage.setItem(STORAGE_KEY, preference); } catch { /* ignore */ }
  }, [preference]);

  // Live-follow the OS while the preference is "system".
  useEffect(() => {
    if (preference !== "system") return;
    if (typeof window === "undefined" || typeof window.matchMedia !== "function") return;
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = () => {
      // Another useTheme instance may have switched to an explicit palette;
      // re-read the stored choice so we never override it from a stale "system".
      if (readPreference() !== "system") return;
      const applied: Theme = mq.matches ? "dark" : "light";
      setResolved(applied);
      document.documentElement.dataset.theme = applied;
    };
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, [preference]);

  const setPreference = useCallback((p: ThemePreference) => setPreferenceState(p), []);
  const setTheme      = useCallback((t: Theme) => setPreferenceState(t), []);
  const toggle = useCallback(() => {
    setPreferenceState(theme === "dark" ? "light" : "dark");
  }, [theme]);

  return { theme, preference, setTheme, setPreference, toggle };
}
