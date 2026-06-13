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
          color:  isDark ? "hsl(var(--muted-foreground))" : "hsl(var(--primary-foreground))",
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
          color:  isDark ? "hsl(var(--primary-foreground))" : "hsl(var(--muted-foreground))",
        }}
      >
        <Moon size={10} strokeWidth={2.5} />
      </span>

      {/* Sliding thumb — brand periwinkle in both modes */}
      <span
        className="absolute rounded-full transition-all duration-200 ease-out"
        style={{
          width:      "20px",
          height:     "20px",
          top:        "3px",
          left:       isDark ? "29px" : "3px",
          background: "linear-gradient(135deg, hsl(var(--primary)), hsl(var(--primary) / 0.82))",
          boxShadow:  "0 2px 6px hsl(var(--primary) / 0.38), inset 0 1px 0 hsl(0 0% 100% / 0.18)",
        }}
      />
    </button>
  );
}
