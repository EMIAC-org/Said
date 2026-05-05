import { Sun, Moon } from "lucide-react";
import type { Theme } from "@/lib/useTheme";

interface Props {
  theme:  Theme;
  toggle: () => void;
}

/**
 * Glass segmented toggle: a frosted pill track with sun + moon icons and a
 * sliding cyan-tinted thumb. Smooth 200ms transition.
 */
export function ThemeToggle({ theme, toggle }: Props) {
  const isDark = theme === "dark";

  return (
    <button
      onClick={toggle}
      aria-label={`Switch to ${isDark ? "light" : "dark"} mode`}
      title={`Switch to ${isDark ? "light" : "dark"} mode`}
      className="relative no-drag rounded-full transition-colors duration-200 flex items-center"
      style={{
        width:      "52px",
        height:     "26px",
        background: "hsl(var(--glass-bg))",
        backdropFilter: "blur(20px)",
        WebkitBackdropFilter: "blur(20px)",
        boxShadow: "inset 0 0 0 1px hsl(var(--glass-stroke))",
        padding:    "3px",
      }}
    >
      {/* Sun icon (left) */}
      <span
        className="absolute flex items-center justify-center transition-colors duration-200"
        style={{
          left:   "6px",
          width:  "14px",
          height: "14px",
          top:    "50%",
          transform: "translateY(-50%)",
          color:  isDark ? "hsl(var(--muted-foreground))" : "hsl(38 95% 52%)",
        }}
      >
        <Sun size={11} strokeWidth={2.5} />
      </span>

      {/* Moon icon (right) */}
      <span
        className="absolute flex items-center justify-center transition-colors duration-200"
        style={{
          right:  "6px",
          width:  "14px",
          height: "14px",
          top:    "50%",
          transform: "translateY(-50%)",
          color:  isDark ? "hsl(var(--primary))" : "hsl(var(--muted-foreground))",
        }}
      >
        <Moon size={10} strokeWidth={2.5} />
      </span>

      {/* Sliding thumb */}
      <span
        className="absolute rounded-full transition-all duration-200 ease-out"
        style={{
          width:      "20px",
          height:     "20px",
          top:        "3px",
          left:       isDark ? "29px" : "3px",
          background: isDark
            ? "linear-gradient(135deg, hsl(0 0% 100%), hsl(0 0% 92%))"
            : "linear-gradient(135deg, hsl(220 28% 12%), hsl(220 28% 6%))",
          boxShadow:  isDark
            ? "0 1px 3px hsl(220 50% 5% / 0.5), 0 0 0 0.5px hsl(0 0% 100% / 0.15) inset"
            : "0 2px 4px hsl(220 28% 10% / 0.30)",
        }}
      />
    </button>
  );
}
