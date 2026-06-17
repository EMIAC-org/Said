"use client";

import { useRef } from "react";
import {
  motion,
  useReducedMotion,
  useScroll,
  useTransform,
} from "framer-motion";
import { ChevronRight } from "lucide-react";
import { hero } from "@/lib/content";
import { HeroBackdrop } from "./HeroBackdrop";
import { HeroDemo } from "./HeroDemo";

/* ------- Small hand-rolled glyphs (no third-party brand-mark imports) ------- */

function AppleGlyph(props: React.SVGProps<SVGSVGElement>) {
  return (
    <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden {...props}>
      <path d="M16.365 12.46c-.02-2.06 1.68-3.05 1.76-3.1-.96-1.4-2.46-1.59-2.99-1.61-1.27-.13-2.48.75-3.13.75-.65 0-1.65-.73-2.71-.71-1.39.02-2.69.81-3.4 2.06-1.45 2.52-.37 6.26 1.05 8.31.69 1 1.51 2.13 2.58 2.09 1.04-.04 1.43-.67 2.69-.67s1.61.67 2.71.65c1.12-.02 1.83-1.02 2.51-2.03.79-1.17 1.12-2.3 1.13-2.36-.02-.01-2.17-.83-2.2-3.38zM14.36 5.34c.57-.69.96-1.65.85-2.6-.82.03-1.82.55-2.41 1.23-.53.61-1 1.59-.88 2.52.92.07 1.86-.47 2.44-1.15z" />
    </svg>
  );
}

function WindowsGlyph(props: React.SVGProps<SVGSVGElement>) {
  return (
    <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden {...props}>
      <rect x="3" y="3" width="8.5" height="8.5" rx="0.5" />
      <rect x="12.5" y="3" width="8.5" height="8.5" rx="0.5" />
      <rect x="3" y="12.5" width="8.5" height="8.5" rx="0.5" />
      <rect x="12.5" y="12.5" width="8.5" height="8.5" rx="0.5" />
    </svg>
  );
}

/* ----------------------------- Hero text block ----------------------------- */

function HeroText() {
  return (
    <div className="relative mx-auto flex h-full max-w-3xl flex-col items-center justify-center px-6 text-center">
      <h1
        className="font-display text-5xl sm:text-6xl md:text-7xl lg:text-8xl tracking-tightest leading-[0.95] text-balance text-white"
        style={{ textShadow: "0 2px 24px rgba(0,0,0,0.45)" }}
      >
        {hero.title}
      </h1>

      <div className="mt-10 flex flex-wrap items-center justify-center gap-3">
        <a
          href={hero.ctaMac.href}
          className="group inline-flex h-11 items-center gap-2 rounded-full bg-white px-5 text-sm font-medium text-ink-900 shadow-[inset_0_1px_0_rgba(255,255,255,0.6),inset_0_-1px_0_rgba(0,0,0,0.12),0_8px_24px_-8px_rgba(0,0,0,0.4)] transition-all duration-200 hover:bg-ink-50 active:translate-y-px"
        >
          <AppleGlyph className="h-4 w-4" />
          {hero.ctaMac.label}
        </a>
        <a
          href={hero.ctaWindows.href}
          className="group inline-flex h-11 items-center gap-2 rounded-full bg-ink-900/80 px-5 text-sm font-medium text-white backdrop-blur-sm border border-white/10 shadow-[inset_0_1px_0_rgba(255,255,255,0.08),0_8px_24px_-8px_rgba(0,0,0,0.4)] transition-all duration-200 hover:bg-ink-900 active:translate-y-px"
        >
          <WindowsGlyph className="h-4 w-4" />
          {hero.ctaWindows.label}
        </a>
      </div>

      <a
        href={hero.iphoneLink.href}
        className="mt-5 inline-flex items-center gap-1 text-sm text-white/70 hover:text-white transition-colors"
      >
        {hero.iphoneLink.label}
        <ChevronRight className="h-3.5 w-3.5" />
      </a>

      <p
        className="mt-12 max-w-md text-sm md:text-base text-white/85 leading-relaxed"
        style={{ textShadow: "0 1px 12px rgba(0,0,0,0.5)" }}
      >
        {hero.subtitle}
      </p>
    </div>
  );
}

/* ---------------------------------- Hero ---------------------------------- */

export function Hero() {
  const reduce = useReducedMotion();
  const ref = useRef<HTMLDivElement>(null);

  const { scrollYProgress } = useScroll({
    target: ref,
    offset: ["start start", "end start"],
  });

  // One shared scroll timeline. Every animation derives from scrollYProgress
  // so the morph reads as a single physical event.
  // Choreography is front-loaded: morph + demo fade complete by progress
  // 0.50, leaving 0.50→1.00 (≈110 vh of scroll) as a stable hold zone where
  // nothing animates — the Play Demo card just sits there, fully readable
  // and clickable. That gives the user real "stop and play" time without
  // having to fight a continuously moving page.
  const scale = useTransform(scrollYProgress, [0, 0.5], [1, 0.62]);
  const radius = useTransform(scrollYProgress, [0, 0.5], [0, 24]);
  const textOpacity = useTransform(scrollYProgress, [0, 0.15], [1, 0]);
  const textY = useTransform(scrollYProgress, [0, 0.15], [0, -12]);
  const overlaysOpacity = useTransform(scrollYProgress, [0, 0.5], [1, 0]);
  // Demo prompt now overlaps the morph — by the time the card stops
  // shrinking, the prompt is already at full opacity. No translucent dead
  // phase between the morph ending and the prompt arriving.
  const demoOpacity = useTransform(scrollYProgress, [0.3, 0.5], [0, 1]);
  const heroPointerEvents = useTransform(scrollYProgress, (v) =>
    v < 0.2 ? "auto" : "none",
  );
  const demoPointerEvents = useTransform(scrollYProgress, (v) =>
    v > 0.45 ? "auto" : "none",
  );

  // Reduced-motion: skip the scroll choreography entirely. Render one
  // framed HeroDemo card inline and let the page scroll normally. HeroDemo
  // is headless now, so the wrapper here provides the aspect-video frame +
  // rounded corners + HeroBackdrop (with overlaysOpacity={0} to match the
  // "collapsed" look — clean gradient + highlight, no silhouette extras).
  if (reduce) {
    return (
      <section
        id="top"
        className="relative pt-24 pb-12"
        style={{ background: "var(--bg)" }}
      >
        <div className="container-page">
          <div className="relative w-full aspect-video overflow-hidden rounded-2xl">
            <HeroBackdrop overlaysOpacity={0} />
            <HeroDemo />
          </div>
        </div>
      </section>
    );
  }

  return (
    <section
      id="top"
      ref={ref}
      className="relative h-[220vh]"
      // Section bg stays the page dark (--bg) across all modes so the
      // area around the morphing hero card reads as one continuous dark
      // page — like superwhisper's hero. The mode-driven sky only paints
      // INSIDE the morphing card via HeroBackdrop's gradient, not on
      // the surrounding section.
      style={{ background: "var(--bg)" }}
      aria-label="Airnote intro"
    >
      <div className="sticky top-0 min-h-screen w-full overflow-hidden">
        <motion.div
          style={{
            scale,
            borderRadius: radius,
            x: "-50%",
            y: "-50%",
          }}
          className="absolute left-1/2 top-1/2 h-screen w-screen overflow-hidden will-change-transform"
        >
          {/* Backdrop fills the entire morphing stage */}
          <HeroBackdrop overlaysOpacity={overlaysOpacity} />

          {/* Hero text — fades out as the user starts scrolling */}
          <motion.div
            style={{
              opacity: textOpacity,
              y: textY,
              pointerEvents: heroPointerEvents,
            }}
            className="absolute inset-0 z-10"
          >
            <HeroText />
          </motion.div>

          {/* HeroDemo — phase UI rendered directly into the morphing stage.
              The stage IS the demo card; no nested frame. */}
          <motion.div
            style={{
              opacity: demoOpacity,
              pointerEvents: demoPointerEvents,
            }}
            className="absolute inset-0 z-20"
          >
            <HeroDemo />
          </motion.div>
        </motion.div>
      </div>
    </section>
  );
}
