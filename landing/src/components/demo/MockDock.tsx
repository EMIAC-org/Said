"use client";

import { cn } from "@/lib/cn";
import { getBrandIcon, isHexDark } from "@/lib/brandIcons";
import { heroDemo } from "@/lib/content";

/* --------------------------- Custom icon glyphs ---------------------------- */

function AirnoteGlyph() {
  // Small isometric cube glyph used as the Airnote dock icon.
  return (
    <svg viewBox="0 0 24 24" width="55%" height="55%" aria-hidden>
      <path
        d="M12 3 L21 8 L12 13 L3 8 Z"
        fill="#ffffff"
        opacity="0.95"
      />
      <path
        d="M3 8 L12 13 L12 21 L3 16 Z"
        fill="#ffffff"
        opacity="0.7"
      />
      <path
        d="M21 8 L12 13 L12 21 L21 16 Z"
        fill="#ffffff"
        opacity="0.55"
      />
    </svg>
  );
}

function NotesGlyph() {
  // Simple lined-paper glyph for the Notes dock icon.
  return (
    <svg viewBox="0 0 24 24" width="60%" height="60%" aria-hidden>
      <line x1="5" y1="8"  x2="19" y2="8"  stroke="#3a2a05" strokeWidth="1.4" strokeLinecap="round" />
      <line x1="5" y1="12" x2="19" y2="12" stroke="#3a2a05" strokeWidth="1.4" strokeLinecap="round" />
      <line x1="5" y1="16" x2="14" y2="16" stroke="#3a2a05" strokeWidth="1.4" strokeLinecap="round" />
    </svg>
  );
}

function SlackGlyph() {
  const brand = getBrandIcon("slack");
  if (!brand) return null;
  const dark = isHexDark(brand.hex);
  return (
    <svg viewBox="0 0 24 24" width="55%" height="55%" aria-hidden>
      <path d={brand.path} fill={dark ? "#ffffff" : `#${brand.hex}`} />
    </svg>
  );
}

/* ---------------------------------- Icon ----------------------------------- */

type IconProps = {
  background: string;
  label: string;
  children: React.ReactNode;
};

function Icon({ background, label, children }: IconProps) {
  return (
    <div
      role="img"
      aria-label={label}
      className="relative h-14 w-14 rounded-[22%] grid place-items-center shrink-0"
      style={{
        background,
        boxShadow:
          "0 4px 8px rgba(0,0,0,0.35), inset 0 1px 0 rgba(255,255,255,0.25)",
      }}
    >
      {children}
    </div>
  );
}

/* ---------------------------------- Dock ----------------------------------- */

export function MockDock({ className }: { className?: string }) {
  return (
    <div
      className={cn(
        "inline-flex items-center gap-3 p-3 rounded-2xl",
        "bg-black/35 backdrop-blur-md border border-white/8",
        "shadow-[0_12px_32px_-12px_rgba(0,0,0,0.5)]",
        className,
      )}
    >
      <Icon
        background="linear-gradient(135deg, #2a2a2d, #0e0e10)"
        label={heroDemo.dock.airnoteLabel}
      >
        <AirnoteGlyph />
      </Icon>
      <Icon
        background="linear-gradient(180deg, #ffffff, #f0f0f0)"
        label={heroDemo.dock.slackLabel}
      >
        <SlackGlyph />
      </Icon>
      <Icon
        background="linear-gradient(180deg, #FFE08A, #F2C75A)"
        label={heroDemo.dock.notesLabel}
      >
        <NotesGlyph />
      </Icon>
    </div>
  );
}
