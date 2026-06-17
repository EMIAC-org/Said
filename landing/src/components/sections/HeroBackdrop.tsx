"use client";

// HeroBackdrop: split-composition hero scene that BLENDS two outdoor
// photos into one continuous sky-and-blossoms scene.
//
//   LEFT edge   — profile portrait against blue sky (Unsplash)
//   CENTER      — mode-driven sky gradient
//   RIGHT edge  — laptop in an outdoor / cherry-blossom scene (Unsplash)
//
// Both photos share an outdoor blue-sky environment so they read as ONE
// continuous scene with the gradient bridging them — instead of two
// unrelated photos pasted in. Each photo is masked at its inner edge so
// the bleed into the gradient is seamless.
//
// Photos fade with `overlaysOpacity` as the hero collapses into HeroDemo,
// leaving the clean base gradient that HeroDemo also uses — collapse
// lands without a visible color shift.

import { motion, type MotionValue, useTransform } from "framer-motion";
import { cn } from "@/lib/cn";

type Props = {
  className?: string;
  /** Drives the fade of the photos + atmospheric overlays. Accepts a
   *  MotionValue (preferred — no React re-renders) or a plain number. */
  overlaysOpacity?: MotionValue<number> | number;
};

export function HeroBackdrop({
  className,
  overlaysOpacity = 1,
}: Props) {
  const overlayMV =
    typeof overlaysOpacity === "number"
      ? overlaysOpacity
      : overlaysOpacity;

  // Grain layer wants a fraction of the overlay opacity (0.08 * o).
  const grainMV = useTransform(() => {
    const o =
      typeof overlayMV === "number" ? overlayMV : overlayMV.get();
    return o * 0.08;
  });

  return (
    <div
      aria-hidden
      className={cn("absolute inset-0 overflow-hidden", className)}
    >
      {/* 1. Mode-driven sky gradient — base layer.
            Same .hero-base-gradient class as HeroDemo so the
            scroll-collapse lands seamlessly. */}
      <div className="absolute inset-0 hero-base-gradient" />

      {/* 2. Top "sunlit" highlight — strength per mode via
            --hero-highlight-strength. */}
      <div className="absolute inset-0 hero-highlight-top" />

      {/* 3. LEFT edge — person profile against blue sky. Masked at its
            inner edge so the silhouette bleeds into the sky gradient. */}
      <motion.img
        src="/hero-person.jpg"
        alt=""
        aria-hidden
        loading="eager"
        decoding="async"
        className="hero-edge-photo hero-edge-photo-person absolute left-0 inset-y-0 h-full w-[40%] max-w-[640px] object-cover object-right pointer-events-none"
        style={{ opacity: overlayMV }}
        onError={(e) => {
          (e.currentTarget as HTMLImageElement).style.display = "none";
        }}
      />

      {/* 4. RIGHT bottom corner — MacBook Air with the Walling screen
            visible. Masked aggressively at the top + left so only the
            laptop itself shows; the desk surface and any sky bleed off
            into the hero sky gradient. mix-blend-mode: luminosity keeps
            the laptop's structure but lets the hero's sky color tint it
            so it feels integrated into the page rather than pasted on. */}
      <motion.img
        src="/hero-laptop.jpg"
        alt=""
        aria-hidden
        loading="eager"
        decoding="async"
        className="hero-edge-photo hero-edge-photo-laptop absolute right-0 bottom-0 h-[62%] w-[34%] max-w-[520px] object-cover object-left-top pointer-events-none"
        style={{ opacity: overlayMV }}
        onError={(e) => {
          (e.currentTarget as HTMLImageElement).style.display = "none";
        }}
      />

      {/* 5. Warm horizon glow at the lower-right (Dusk only — Day/Night
            set --hero-warm-strength to 0). */}
      <motion.div
        className="absolute inset-0 hero-horizon-warm"
        style={{ opacity: overlayMV }}
      />

      {/* 6. Film grain — keeps the scene from reading as a flat gradient. */}
      <motion.div
        className="absolute inset-0 mix-blend-overlay pointer-events-none"
        style={{
          opacity: grainMV,
          backgroundImage:
            "url(\"data:image/svg+xml,%3Csvg viewBox='0 0 200 200' xmlns='http://www.w3.org/2000/svg'%3E%3Cfilter id='n'%3E%3CfeTurbulence type='fractalNoise' baseFrequency='0.85' numOctaves='2' stitchTiles='stitch'/%3E%3C/filter%3E%3Crect width='100%25' height='100%25' filter='url(%23n)'/%3E%3C/svg%3E\")",
        }}
      />
    </div>
  );
}
