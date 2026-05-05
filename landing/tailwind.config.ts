import type { Config } from 'tailwindcss';
import { colors, fontFamily, fontSize, radius, shadow, spacing } from './lib/tokens';

const config: Config = {
  content: ['./app/**/*.{ts,tsx}', './components/**/*.{ts,tsx}'],
  theme: {
    extend: {
      colors: {
        accent: colors.accent,
        'accent-text': colors.accentText,
        background: colors.background,
        ink: colors.ink,
        text: colors.text,
        muted: colors.muted,
      },
      fontFamily: {
        sans: fontFamily.sans as unknown as string[],
      },
      fontSize: fontSize as unknown as NonNullable<Config['theme']>['fontSize'],
      spacing: {
        'token-xs': spacing.xs,
        'token-sm': spacing.sm,
        'token-md': spacing.md,
        'token-lg': spacing.lg,
        'token-xl': spacing.xl,
      },
      borderRadius: {
        'token-sm': radius.sm,
        'token-md': radius.md,
        'token-lg': radius.lg,
      },
      boxShadow: {
        button: shadow.button,
        card: shadow.card,
        glow: shadow.glow,
      },
    },
  },
  plugins: [],
};

export default config;
