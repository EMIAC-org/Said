import type { Config } from "tailwindcss";

// Ink tokens are wired through CSS variables (RGB triplets) so they invert
// automatically when `<html data-theme="light|dark">` flips. See globals.css
// for the per-theme palettes.
const config: Config = {
  content: ["./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        ink: {
          DEFAULT: "rgb(var(--ink-default-rgb) / <alpha-value>)",
          50: "rgb(var(--ink-50-rgb) / <alpha-value>)",
          100: "rgb(var(--ink-100-rgb) / <alpha-value>)",
          200: "rgb(var(--ink-200-rgb) / <alpha-value>)",
          300: "rgb(var(--ink-300-rgb) / <alpha-value>)",
          400: "rgb(var(--ink-400-rgb) / <alpha-value>)",
          500: "rgb(var(--ink-500-rgb) / <alpha-value>)",
          600: "rgb(var(--ink-600-rgb) / <alpha-value>)",
          700: "rgb(var(--ink-700-rgb) / <alpha-value>)",
          800: "rgb(var(--ink-800-rgb) / <alpha-value>)",
          900: "rgb(var(--ink-900-rgb) / <alpha-value>)",
        },
        // Brand chrome — matches the Airnote desktop app palette.
        // Periwinkle as primary, indigo-200/400 as soft/deep variants,
        // green-400 reserved for success / streak states.
        accent: {
          DEFAULT: "#a5b4fc",
          soft: "#c7d2fe",
          deep: "#818cf8",
          success: "#4ade80",
        },
      },
      fontFamily: {
        sans: ["var(--font-inter)", "system-ui", "sans-serif"],
        display: ["var(--font-display)", "var(--font-inter)", "system-ui", "sans-serif"],
      },
      letterSpacing: {
        "tightest": "-0.04em",
      },
      borderRadius: {
        xl: "16px",
        "2xl": "24px",
      },
      keyframes: {
        spinSlow: {
          "0%": { transform: "rotate(0deg)" },
          "100%": { transform: "rotate(360deg)" },
        },
        marquee: {
          "0%": { transform: "translateX(0)" },
          "100%": { transform: "translateX(-50%)" },
        },
        pulseRing: {
          "0%, 100%": { boxShadow: "0 0 0 0 rgba(165,180,252,0.4)" },
          "50%": { boxShadow: "0 0 0 14px rgba(165,180,252,0)" },
        },
      },
      animation: {
        spinSlow: "spinSlow 40s linear infinite",
        marquee: "marquee 40s linear infinite",
        pulseRing: "pulseRing 2.2s ease-out infinite",
      },
    },
  },
  plugins: [],
};

export default config;
