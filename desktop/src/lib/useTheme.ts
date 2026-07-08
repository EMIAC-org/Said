import { useCallback, useEffect, useState } from "react";

export type Theme = "dark" | "light";
export type ThemePreference = Theme | "system";

const STORAGE_KEY = "vp-theme";

function systemTheme(): Theme {
  if (typeof window === "undefined" || !window.matchMedia) return "dark";
  return window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
}

function isThemePreference(value: string | null | undefined): value is ThemePreference {
  return value === "dark" || value === "light" || value === "system";
}

function resolveTheme(preference: ThemePreference): Theme {
  return preference === "system" ? systemTheme() : preference;
}

/**
 * Read/write theme to localStorage and `document.documentElement.dataset.theme`.
 * The initial value is ALSO synced from a small inline script in index.html
 * (no-flash bootstrap). Defaults to the system appearance.
 */
export function useTheme(): {
  theme:  Theme;
  preference: ThemePreference;
  toggle: () => void;
  setTheme: (t: ThemePreference) => void;
} {
  const [preference, setPreference] = useState<ThemePreference>(() => {
    if (typeof localStorage === "undefined") return "system";
    try {
      const stored = localStorage.getItem(STORAGE_KEY);
      return isThemePreference(stored) ? stored : "system";
    } catch {
      return "system";
    }
  });
  const [theme, setResolvedTheme] = useState<Theme>(() => {
    if (typeof document === "undefined") return resolveTheme(preference);
    const bootstrapped = document.documentElement.dataset.theme;
    return bootstrapped === "light" || bootstrapped === "dark" ? bootstrapped : resolveTheme(preference);
  });

  useEffect(() => {
    const apply = () => {
      const resolved = resolveTheme(preference);
      setResolvedTheme(resolved);
      document.documentElement.dataset.theme = resolved;
    };

    apply();
    try { localStorage.setItem(STORAGE_KEY, preference); } catch { /* ignore */ }

    if (preference !== "system" || !window.matchMedia) return;
    const media = window.matchMedia("(prefers-color-scheme: light)");
    media.addEventListener("change", apply);
    return () => media.removeEventListener("change", apply);
  }, [preference]);

  const toggle = useCallback(() => {
    setPreference((current) => (resolveTheme(current) === "dark" ? "light" : "dark"));
  }, []);

  return { theme, preference, toggle, setTheme: setPreference };
}
