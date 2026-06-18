"use client";

import { useEffect, useState } from "react";
import { motion, useReducedMotion } from "framer-motion";
import { nav } from "@/lib/content";
import { cn } from "@/lib/cn";

function LogoGlyph() {
  // Small filled triangle/peak as the Download pill's leading glyph.
  return (
    <svg viewBox="0 0 24 24" aria-hidden className="h-3.5 w-3.5">
      <path d="M12 4 L22 20 L2 20 Z" fill="currentColor" />
    </svg>
  );
}

/**
 * Floating pill nav. Hidden on first load while the cinematic hero is
 * full-bleed; fades in once the user has scrolled past ~65% of the first
 * viewport (so it appears as the hero collapse is well underway). Stays
 * pinned to the top thereafter.
 */
export function Nav() {
  const reduce = useReducedMotion();
  const [visible, setVisible] = useState(false);

  useEffect(() => {
    if (reduce) {
      setVisible(true);
      return;
    }
    const onScroll = () => {
      setVisible(window.scrollY > window.innerHeight * 0.65);
    };
    onScroll();
    window.addEventListener("scroll", onScroll, { passive: true });
    return () => window.removeEventListener("scroll", onScroll);
  }, [reduce]);

  return (
    <motion.header
      initial={false}
      animate={{
        opacity: visible ? 1 : 0,
        y: visible ? 0 : -16,
        pointerEvents: visible ? "auto" : "none",
      }}
      transition={{ duration: 0.45, ease: [0.22, 1, 0.36, 1] }}
      className="fixed left-1/2 top-5 z-40 -translate-x-1/2"
      aria-hidden={!visible}
    >
      <nav
        className={cn(
          "flex h-12 items-center gap-1 rounded-full",
          "bg-black/55 backdrop-blur-md border border-white/10",
          "shadow-[0_12px_32px_-16px_rgba(0,0,0,0.6)]",
          "px-2",
        )}
        aria-label="Primary"
      >
        {nav.links.map((l) => (
          <a
            key={l.label}
            href={l.href}
            className="h-9 inline-flex items-center rounded-full px-4 text-sm text-white/75 hover:text-white hover:bg-white/8 transition-colors"
          >
            {l.label}
          </a>
        ))}
        <a
          href={nav.cta.href}
          className="ml-1 inline-flex h-9 items-center gap-1.5 rounded-full bg-white px-4 text-sm font-medium text-ink-900 hover:bg-ink-50 transition-colors"
        >
          <LogoGlyph />
          {nav.cta.label}
        </a>
      </nav>
    </motion.header>
  );
}
