import { useCallback, useEffect, useState } from "react";

export type Theme = "dark" | "light";

const STORAGE_KEY = "vp-theme";

/**
 * Read/write theme to localStorage and `document.documentElement.dataset.theme`.
 * The initial value is ALSO synced from a small inline script in index.html
 * (no-flash bootstrap). Defaults to dark.
 */
export function useTheme(): {
  theme:  Theme;
  toggle: () => void;
  setTheme: (t: Theme) => void;
} {
  const [theme, setThemeState] = useState<Theme>(() => {
    if (typeof document === "undefined") return "dark";
    return (document.documentElement.dataset.theme as Theme) ?? "dark";
  });

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    try { localStorage.setItem(STORAGE_KEY, theme); } catch { /* ignore */ }
  }, [theme]);

  const toggle = useCallback(() => {
    setThemeState((t) => (t === "dark" ? "light" : "dark"));
  }, []);

  return { theme, toggle, setTheme: setThemeState };
}
