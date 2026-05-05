/**
 * Centralized design tokens — the single source of truth for the Said landing page.
 * Tailwind config and runtime code both consume these values.
 */

export const colors = {
  accent: '#67beff',
  accentText: '#0000ee',
  background: '#fafbfc',
  text: '#292c3d',
  muted: '#7a7d88',
  ink: '#000000',
} as const;

export const fontFamily = {
  sans: [
    'PP Neue Montreal Book',
    'PP Neue Montreal',
    'var(--font-sans)',
    'Inter Tight',
    'Inter',
    '-apple-system',
    'BlinkMacSystemFont',
    'Segoe UI',
    'sans-serif',
  ],
} as const;

export const fontSize = {
  caption: ['0.75rem', { lineHeight: '1.1rem' }],
  label: ['0.875rem', { lineHeight: '1.25rem' }],
  body: ['1.5rem', { lineHeight: '2.4rem' }],
  h4: ['1.9375rem', { lineHeight: '2.4rem' }],
  h3: ['2.4375rem', { lineHeight: '2.875rem' }],
  h2: ['3.125rem', { lineHeight: '3.625rem' }],
  h1: ['4rem', { lineHeight: '4.5rem', letterSpacing: '-0.01em' }],
} as const;

export const spacing = {
  xs: '0.625rem',
  sm: '1.25rem',
  md: '2.5rem',
  lg: '4rem',
  xl: '6.625rem',
} as const;

export const radius = {
  sm: '0.25rem',
  md: '0.5rem',
  lg: '0.75rem',
} as const;

export const shadow = {
  button:
    '0 1px 2px rgba(13, 71, 161, 0.18), inset 0 1px 0 rgba(255, 255, 255, 0.35)',
  card: '0 8px 24px rgba(41, 44, 61, 0.06)',
  glow: '0 0 24px rgba(103, 190, 255, 0.45)',
} as const;
