"use client";

import { motion, useReducedMotion } from "framer-motion";
import {
  WifiOff,
  BookOpen,
  Sliders,
  Globe,
  Clipboard,
  Users,
  FileText,
  Hand,
  Command,
  Sparkles,
  type LucideIcon,
} from "lucide-react";
import { features } from "@/lib/content";
import {
  VISUALS,
  type VisualKey,
} from "@/components/sections/FeatureShowcaseVisuals";
import { cn } from "@/lib/cn";

/* ─────────────────────────────── Icon map ────────────────────────────────── */

const SHOWCASE_ICONS: Record<string, LucideIcon> = {
  WifiOff,
  BookOpen,
  Sliders,
  Globe,
  Clipboard,
  Users,
};

const CALLOUT_ICONS: Record<string, LucideIcon> = {
  FileText,
  Hand,
  Command,
  Sparkles,
};

/* ─────────────────────────────── Accent map ──────────────────────────────── */

type Accent = "violet" | "emerald" | "rose" | "sky" | "green" | "cyan";

const ACCENT_TITLE: Record<Accent, string> = {
  violet: "text-violet-300",
  emerald: "text-emerald-300",
  rose: "text-rose-300",
  sky: "text-sky-300",
  green: "text-green-300",
  cyan: "text-cyan-300",
};

const ACCENT_GLOW: Record<Accent, string> = {
  violet: "feature-glow-violet",
  emerald: "feature-glow-emerald",
  rose: "feature-glow-rose",
  sky: "feature-glow-sky",
  green: "feature-glow-green",
  cyan: "feature-glow-cyan",
};

/* ─────────────────────────────── Showcase card ───────────────────────────── */

function ShowcaseCard({
  accent,
  icon,
  visual,
  title,
  body,
  tagline,
}: {
  accent: Accent;
  icon: string;
  visual: VisualKey;
  title: string;
  body: string;
  tagline: string;
}) {
  const Visual = VISUALS[visual];
  const Icon = SHOWCASE_ICONS[icon] ?? Sparkles;
  return (
    <div
      className={cn(
        "feature-card group relative w-full aspect-[4/5] rounded-2xl hairline overflow-hidden",
        ACCENT_GLOW[accent],
      )}
    >
      {/* Visual fills the entire card — no separate text panel beneath. */}
      <Visual />

      {/* Bottom gradient — fades the visual into a near-black backdrop so
          the title + body + tagline read cleanly while the colored image
          still bleeds through the upper part of the text area. */}
      <div
        aria-hidden
        className="absolute inset-x-0 bottom-0 h-[58%] pointer-events-none"
        style={{
          background:
            "linear-gradient(180deg, transparent 0%, rgba(0,0,0,0.55) 35%, rgba(0,0,0,0.95) 100%)",
        }}
      />

      {/* Text overlay — title + body + tagline embedded into the visual. */}
      <div className="absolute inset-x-0 bottom-0 p-5 md:p-6">
        <div className="flex items-center gap-2">
          <Icon
            className={cn("h-[17px] w-[17px] shrink-0", ACCENT_TITLE[accent])}
            strokeWidth={2}
          />
          <h3
            className={cn(
              "font-display text-lg md:text-[19px] tracking-tight leading-tight",
              ACCENT_TITLE[accent],
            )}
          >
            {title}
          </h3>
        </div>
        <p className="mt-2 text-[13px] text-ink-200 leading-snug">{body}</p>
        <p className="mt-1.5 text-[13px] font-semibold text-ink-50 leading-snug">
          {tagline}
        </p>
      </div>
    </div>
  );
}

/* ─────────────────────────────── Callout card ────────────────────────────── */

function CalloutCard({
  icon,
  title,
  body,
  badge,
}: {
  icon: string;
  title: string;
  body: string;
  badge?: string;
}) {
  const Icon = CALLOUT_ICONS[icon] ?? Sparkles;
  return (
    <div className="feature-card group h-full rounded-xl p-1 hover:bg-ink-800/40">
      <div className="flex items-center gap-2">
        <Icon className="h-5 w-5 text-ink-100" strokeWidth={1.75} />
        <h3 className="text-[15px] font-semibold text-ink-50 flex items-center gap-2 flex-wrap">
          {title}
          {badge && (
            <span
              className="inline-block rounded-md px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-wider"
              style={{
                background: "#a5b4fc",
                color: "#1e1b4b",
              }}
            >
              {badge}
            </span>
          )}
        </h3>
      </div>
      <p className="mt-3 text-[13.5px] text-ink-200 leading-relaxed max-w-[28ch]">
        {body}
      </p>
    </div>
  );
}

/* ──────────────────────────────── Section ────────────────────────────────── */

export function FeatureGrid() {
  const reduce = useReducedMotion();

  return (
    <section
      id="features"
      aria-labelledby="features-title"
      className="relative py-24 md:py-32"
    >
      {/* Section header — stays inside container-page */}
      <motion.div
        initial={reduce ? false : { opacity: 0, y: 16 }}
        whileInView={{ opacity: 1, y: 0 }}
        viewport={{ once: true, margin: "-10% 0px" }}
        transition={{ duration: 0.6, ease: [0.22, 1, 0.36, 1] }}
        className="container-page"
      >
        <div className="max-w-3xl">
          <p className="feature-eyebrow text-xs md:text-sm uppercase tracking-[0.18em] font-semibold mb-4">
            {features.eyebrow}
          </p>
          <h2
            id="features-title"
            className="font-display text-4xl md:text-5xl lg:text-6xl tracking-tightest leading-[1.05] text-balance"
          >
            {features.title}
          </h2>
          <p className="mt-6 text-lg md:text-xl text-ink-200 max-w-2xl leading-relaxed text-balance">
            {features.subtitle}
          </p>
        </div>
      </motion.div>

      {/* ─────────────────── Showcase row (6 cards, carousel) ─────────────── */}
      {/* Full-bleed: cards extend off the container edges and the user scrolls
          horizontally. Edge fade hints at more cards beyond the viewport. */}
      <div className="mt-14 md:mt-16 fade-edges-x">
        <motion.ul
          initial="hidden"
          whileInView="show"
          viewport={{ once: true, margin: "-10% 0px" }}
          variants={{
            hidden: {},
            show: { transition: { staggerChildren: 0.06 } },
          }}
          className="scroll-snap-x flex gap-5 pl-6 pr-6 md:pl-[max(1.5rem,calc((100vw-72rem)/2+1.5rem))] md:pr-[max(1.5rem,calc((100vw-72rem)/2+1.5rem))]"
        >
          {features.showcase.map((card) => (
            <motion.li
              key={card.id}
              variants={{
                hidden: reduce ? { opacity: 1 } : { opacity: 0, y: 20 },
                show: { opacity: 1, y: 0 },
              }}
              transition={{ duration: 0.6, ease: [0.22, 1, 0.36, 1] }}
              className="scroll-snap-card shrink-0 w-[270px] md:w-[300px]"
            >
              <ShowcaseCard
                accent={card.accent as Accent}
                icon={card.icon}
                visual={card.visual as VisualKey}
                title={card.title}
                body={card.body}
                tagline={card.tagline}
              />
            </motion.li>
          ))}
        </motion.ul>
      </div>

      {/* ───────────────────────── Callout row (4 small) ──────────────────── */}
      <div className="container-page mt-16 md:mt-20">
        <motion.ul
          initial="hidden"
          whileInView="show"
          viewport={{ once: true, margin: "-10% 0px" }}
          variants={{
            hidden: {},
            show: { transition: { staggerChildren: 0.06 } },
          }}
          className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-6 md:gap-8"
        >
          {features.callouts.map((c) => (
            <motion.li
              key={c.title}
              variants={{
                hidden: reduce ? { opacity: 1 } : { opacity: 0, y: 12 },
                show: { opacity: 1, y: 0 },
              }}
              transition={{ duration: 0.5, ease: [0.22, 1, 0.36, 1] }}
            >
              <CalloutCard
                icon={c.icon}
                title={c.title}
                body={c.body}
                badge={"badge" in c ? (c as { badge?: string }).badge : undefined}
              />
            </motion.li>
          ))}
        </motion.ul>
      </div>
    </section>
  );
}
