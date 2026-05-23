/**
 * AutoNote brand mark — a 4-bar audio waveform.
 *
 * Renders crisp from 14px (macOS menu bar) up to 64px+ (onboarding brand
 * canvas). Uses `currentColor` so the mark tints correctly in any context
 * without re-rendering. The asymmetric peak (heights 7/15/19/11) reads as
 * "voice rising and resolving" rather than a static equalizer.
 */
export function BrandMark({
  size      = 22,
  className,
}: {
  size?:     number;
  /** Kept for compatibility with existing callers — no longer used internally. */
  idSuffix?: string;
  className?: string;
}) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      className={`bars-mark ${className ?? ""}`.trim()}
      xmlns="http://www.w3.org/2000/svg"
      aria-hidden="true"
    >
      <rect x="3"  y="8.5" width="3" height="7"  rx="1.5" />
      <rect x="8"  y="4.5" width="3" height="15" rx="1.5" />
      <rect x="13" y="2.5" width="3" height="19" rx="1.5" />
      <rect x="18" y="6.5" width="3" height="11" rx="1.5" />
    </svg>
  );
}
