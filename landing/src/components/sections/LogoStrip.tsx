"use client";

import { motion, useReducedMotion } from "framer-motion";
import { logoStrip } from "@/lib/content";
import { getBrandIcon } from "@/lib/brandIcons";

/* ─────────────────────────── BrandGlyph helper ───────────────────────────── */
/* Renders a single brand mark from simple-icons as a monochrome SVG. The
   glyph defaults to grayscale (currentColor on ink-200) so the band reads
   as one tonal stripe; on hover the path animates back to the brand color
   for a touch of warmth. */

function BrandGlyph({
  slug,
  name,
}: {
  slug: string;
  name: string;
}) {
  const icon = getBrandIcon(slug);
  if (!icon) {
    // Fallback: render the brand name in wordmark style so a missing slug
    // never breaks the row layout.
    return (
      <span
        className="font-display tracking-[0.18em] text-sm text-ink-200 uppercase"
        aria-label={name}
      >
        {name}
      </span>
    );
  }
  return (
    <svg
      role="img"
      aria-label={name}
      viewBox="0 0 24 24"
      className="h-5 md:h-6 w-auto text-ink-200 transition-colors duration-300 hover:text-ink-50"
      fill="currentColor"
    >
      <title>{name}</title>
      <path d={icon.path} />
    </svg>
  );
}

/* ──────────────────────────────── Section ────────────────────────────────── */

export function LogoStrip() {
  const reduce = useReducedMotion();

  return (
    <section
      aria-labelledby="logo-strip-title"
      className="relative py-16 md:py-20 overflow-hidden"
    >
      {/* Soft ambient accent glow behind the headline — embeds the band
          into the surrounding page bg instead of boxing it off with a
          border + surface tint. Periwinkle ties to the brand accent
          without dominating; it fades out toward the edges so the
          section reads as one continuous breath of the page. */}
      <div
        aria-hidden
        className="absolute inset-x-0 -top-8 h-[70%] pointer-events-none"
        style={{
          background:
            "radial-gradient(ellipse 45% 80% at 50% 30%, rgba(165, 180, 252, 0.07) 0%, rgba(165, 180, 252, 0.025) 35%, transparent 75%)",
        }}
      />

      {/* Faint underline accent — a single hairline of light at the very
          bottom of the section, fading in and out at the gutters. Acts
          as a visual anchor for the logos without forming a hard border. */}
      <div
        aria-hidden
        className="absolute inset-x-0 bottom-0 h-px pointer-events-none"
        style={{
          background:
            "linear-gradient(90deg, transparent 0%, rgba(165, 180, 252, 0.12) 50%, transparent 100%)",
        }}
      />

      <div className="container-page relative">
        <motion.p
          initial={reduce ? false : { opacity: 0, y: 8 }}
          whileInView={{ opacity: 1, y: 0 }}
          viewport={{ once: true, margin: "-15% 0px" }}
          transition={{ duration: 0.5, ease: [0.22, 1, 0.36, 1] }}
          className="text-center text-[11px] uppercase tracking-[0.22em] text-ink-300 mb-3"
        >
          {logoStrip.eyebrow}
        </motion.p>

        <motion.h2
          id="logo-strip-title"
          initial={reduce ? false : { opacity: 0, y: 12 }}
          whileInView={{ opacity: 1, y: 0 }}
          viewport={{ once: true, margin: "-15% 0px" }}
          transition={{ duration: 0.6, delay: 0.05, ease: [0.22, 1, 0.36, 1] }}
          className="font-display text-2xl md:text-3xl tracking-tight leading-tight text-center text-balance text-ink-50"
        >
          {logoStrip.title}
        </motion.h2>

        <motion.p
          initial={reduce ? false : { opacity: 0, y: 8 }}
          whileInView={{ opacity: 1, y: 0 }}
          viewport={{ once: true, margin: "-15% 0px" }}
          transition={{ duration: 0.6, delay: 0.1, ease: [0.22, 1, 0.36, 1] }}
          className="text-center text-sm text-ink-200 mt-3 max-w-md mx-auto leading-relaxed"
        >
          {logoStrip.subtitle}
        </motion.p>

        <motion.ul
          initial="hidden"
          whileInView="show"
          viewport={{ once: true, margin: "-15% 0px" }}
          variants={{
            hidden: {},
            show: { transition: { staggerChildren: 0.05, delayChildren: 0.2 } },
          }}
          className="mt-9 md:mt-10 grid grid-cols-3 md:grid-cols-6 items-center justify-items-center gap-x-8 gap-y-7"
        >
          {logoStrip.logos.map(({ slug, name }) => (
            <motion.li
              key={slug}
              variants={{
                hidden: reduce ? { opacity: 1 } : { opacity: 0, y: 8 },
                show: { opacity: 1, y: 0 },
              }}
              transition={{ duration: 0.5, ease: [0.22, 1, 0.36, 1] }}
              className="flex items-center justify-center"
            >
              <BrandGlyph slug={slug} name={name} />
            </motion.li>
          ))}
        </motion.ul>
      </div>
    </section>
  );
}
