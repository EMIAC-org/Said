/**
 * The Said brand mark — mint-green rounded tile with double curly-quote
 * glyphs and a voice baseline wave. Single source of truth used by the
 * sidebar header and auth screens so the brand reads identically wherever
 * it appears.
 *
 * `size` defaults to 32 (sidebar). Use 56–72 for hero placements.
 * The gradient ID is keyed off `idSuffix` so multiple BrandMarks on the
 * same page don't collide.
 */
export function BrandMark({
  size      = 32,
  idSuffix  = "default",
  className,
}: {
  size?:     number;
  idSuffix?: string;
  className?: string;
}) {
  const gradId = `brand-grad-${idSuffix}`;
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 32 32"
      fill="none"
      className={className}
      xmlns="http://www.w3.org/2000/svg"
    >
      <defs>
        <linearGradient id={gradId} x1="0" y1="0" x2="32" y2="32" gradientUnits="userSpaceOnUse">
          <stop offset="0%"  stopColor="hsl(105 80% 72%)" />
          <stop offset="100%" stopColor="hsl(160 70% 55%)" />
        </linearGradient>
      </defs>
      <rect width="32" height="32" rx="9" fill={`url(#${gradId})`} />
      <path
        d="M 9.5 11 C 9.5 9 11 8 12.5 8 L 12.5 9.5 C 11.7 9.5 11.2 10 11.2 10.7 L 12.7 10.7 C 13.4 10.7 13.7 11.2 13.7 12 L 13.7 14.5 C 13.7 15.3 13.2 15.8 12.4 15.8 L 10.8 15.8 C 10 15.8 9.5 15.3 9.5 14.5 Z M 17.5 11 C 17.5 9 19 8 20.5 8 L 20.5 9.5 C 19.7 9.5 19.2 10 19.2 10.7 L 20.7 10.7 C 21.4 10.7 21.7 11.2 21.7 12 L 21.7 14.5 C 21.7 15.3 21.2 15.8 20.4 15.8 L 18.8 15.8 C 18 15.8 17.5 15.3 17.5 14.5 Z"
        fill="white"
      />
      <path
        d="M 9 21 Q 12 19 14 21 T 19 21 T 23 21"
        stroke="white"
        strokeWidth="1.5"
        strokeLinecap="round"
        strokeOpacity="0.65"
        fill="none"
      />
    </svg>
  );
}
