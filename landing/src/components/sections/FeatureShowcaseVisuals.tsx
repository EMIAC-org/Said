"use client";

/* ========================================================================== */
/*  Feature showcase visuals — six CSS / SVG mock UIs (two photo-backed)      */
/*  that sit at the top of the large showcase cards in FeatureGrid.           */
/*                                                                            */
/*  IP: all UI mocks are built from scratch — no scraping from any third-     */
/*  party screenshot host. The two photos used (mountain landscape +          */
/*  laptop video conference) are from Unsplash under their commercial-use     */
/*  license. See public/feature-mountain.jpg and public/feature-meeting.jpg.  */
/*                                                                            */
/*  Each visual fills its parent via absolute inset-0 so the parent card     */
/*  controls aspect ratio. Visuals scale via flex + percentages so they      */
/*  reflow cleanly at any card width.                                        */
/*                                                                            */
/*  Two visuals are INTERACTIVE (iteration 16):                              */
/*    - WifiToggleVisual: clickable Wi-Fi toggle (role=switch)               */
/*    - ModePickerVisual: clickable Voice/Message/Email tiles (radiogroup)   */
/*  The others stay decorative (aria-hidden).                                */
/* ========================================================================== */

import { useState } from "react";
import { motion, useReducedMotion } from "framer-motion";
import {
  Mic,
  MessageCircle,
  Mail,
  Trash2,
  CornerDownLeft,
  Command,
  Plus,
  Smile,
  Send,
} from "lucide-react";

type VisualProps = { className?: string };

/* ─────────────────────────── 1. Works offline ────────────────────────────── */
/* Photo-backed: mountain landscape at dusk + INTERACTIVE iOS Wi-Fi toggle
   floating centered on a frosted glass card. The toggle is a real
   role="switch" button — click it (or Space when focused) to flip on/off.
   The thumb slides smoothly between positions; the pill background tints
   green when on. CSS gradient fallback if the photo fails to load. */

export function WifiToggleVisual({ className }: VisualProps) {
  const reduce = useReducedMotion();
  const [on, setOn] = useState(false);

  return (
    <div
      className={className}
      style={{
        position: "absolute",
        inset: 0,
        // CSS fallback gradient — visible until the photo paints, and also
        // serves as the always-present base if /feature-mountain.jpg ever
        // fails to load.
        background:
          "linear-gradient(180deg, #2a1a45 0%, #5a3577 35%, #8e4a72 70%, #1a0e25 100%)",
        overflow: "hidden",
      }}
    >
      {/* Photo backdrop */}
      {/* eslint-disable-next-line @next/next/no-img-element */}
      <img
        src="/feature-mountain.jpg"
        alt=""
        loading="lazy"
        decoding="async"
        aria-hidden
        className="absolute inset-0 h-full w-full object-cover pointer-events-none"
        onError={(e) => {
          (e.currentTarget as HTMLImageElement).style.display = "none";
        }}
      />

      {/* Cool tint over photo so the toggle has contrast room */}
      <div
        aria-hidden
        className="absolute inset-0 pointer-events-none"
        style={{
          background:
            "linear-gradient(180deg, rgba(20,15,40,0.15) 0%, rgba(20,10,30,0.45) 100%)",
        }}
      />

      {/* Interactive toggle */}
      <div className="absolute left-1/2 top-1/2 -translate-x-1/2 -translate-y-1/2 w-[80%] max-w-[260px]">
        <div
          className="rounded-3xl px-5 py-4 flex items-center justify-between"
          style={{
            background: "rgba(255,255,255,0.16)",
            backdropFilter: "blur(14px) saturate(140%)",
            WebkitBackdropFilter: "blur(14px) saturate(140%)",
            border: "1px solid rgba(255,255,255,0.22)",
            boxShadow: "0 10px 30px -8px rgba(0,0,0,0.4)",
          }}
        >
          <span className="text-[15px] md:text-base font-medium text-white tracking-wide">
            Wi-Fi
          </span>
          <button
            type="button"
            role="switch"
            aria-checked={on}
            aria-label="Wi-Fi"
            onClick={() => setOn((v) => !v)}
            className="relative h-7 w-12 rounded-full transition-colors duration-300 focus-visible:outline-none"
            style={{
              background: on
                ? "rgba(52, 211, 153, 0.95)"
                : "rgba(255,255,255,0.32)",
            }}
          >
            <motion.span
              aria-hidden
              className="absolute top-0.5 h-6 w-6 rounded-full bg-white"
              style={{
                boxShadow: "0 2px 4px rgba(0,0,0,0.25)",
              }}
              animate={{ left: on ? "calc(100% - 26px)" : "2px" }}
              transition={
                reduce
                  ? { duration: 0 }
                  : { duration: 0.28, ease: [0.22, 1, 0.36, 1] }
              }
            />
          </button>
        </div>
      </div>
    </div>
  );
}

/* ─────────────────────────── 2. Use your own words ──────────────────────── */
/* Emerald gradient + "Add a new word" form panel with ⌘+Enter chip nested
   inside the Create button, then a list of saved words with trash icons. */

export function VocabPanelVisual({ className }: VisualProps) {
  const words = ["EBITDA", "C'est la vie", "Øystein"];
  return (
    <div
      aria-hidden
      className={className}
      style={{
        position: "absolute",
        inset: 0,
        background:
          "linear-gradient(160deg, #022c22 0%, #064e3b 50%, #022c22 100%)",
      }}
    >
      {/* Subtle paper-texture sheen */}
      <div
        style={{
          position: "absolute",
          inset: 0,
          background:
            "repeating-linear-gradient(180deg, rgba(255,255,255,0.018) 0 3px, transparent 3px 9px)",
        }}
      />
      {/* Top highlight */}
      <div
        style={{
          position: "absolute",
          inset: 0,
          background:
            "radial-gradient(ellipse 60% 40% at 30% 0%, rgba(94, 234, 212, 0.18), transparent 60%)",
        }}
      />

      <div className="absolute left-1/2 top-1/2 -translate-x-1/2 -translate-y-1/2 w-[86%] max-w-[290px]">
        <div
          className="rounded-2xl p-4"
          style={{
            background: "rgba(2, 30, 24, 0.72)",
            backdropFilter: "blur(10px)",
            WebkitBackdropFilter: "blur(10px)",
            border: "1px solid rgba(94, 234, 212, 0.22)",
          }}
        >
          <div className="text-[10px] uppercase tracking-[0.18em] text-emerald-200/70 mb-2">
            Add a new word
          </div>

          {/* Focused input row */}
          <div
            className="h-9 rounded-lg px-3 flex items-center text-[12px] text-white/55"
            style={{
              background: "rgba(255,255,255,0.04)",
              border: "1.5px solid rgba(94, 234, 212, 0.55)",
              boxShadow: "0 0 0 4px rgba(94, 234, 212, 0.08)",
            }}
          >
            <span className="inline-block w-px h-3.5 bg-emerald-200/90 mr-0.5" />
            Word
          </div>

          {/* Create button with nested ⌘ + Enter chip */}
          <div className="mt-2.5 flex items-center justify-end gap-1.5">
            <span
              className="h-8 px-3 rounded-md flex items-center gap-1 text-[12px] font-medium text-emerald-950"
              style={{ background: "rgba(94, 234, 212, 0.92)" }}
            >
              Create
              <span
                className="ml-1 flex items-center gap-0.5 h-5 px-1.5 rounded text-[11px]"
                style={{
                  background: "rgba(2, 30, 24, 0.32)",
                  color: "rgba(2, 30, 24, 0.85)",
                }}
              >
                <Command className="h-2.5 w-2.5" strokeWidth={2.5} />
                <CornerDownLeft
                  className="h-2.5 w-2.5"
                  strokeWidth={2.5}
                />
              </span>
            </span>
          </div>

          {/* Word list */}
          <ul className="mt-3 space-y-1">
            {words.map((w, i) => (
              <li
                key={w}
                className="flex items-center justify-between rounded-md px-2.5 py-1.5"
                style={{
                  background:
                    i === 0
                      ? "rgba(94, 234, 212, 0.10)"
                      : "transparent",
                  borderBottom:
                    i < words.length - 1
                      ? "1px solid rgba(94, 234, 212, 0.08)"
                      : "none",
                }}
              >
                <span className="text-[12.5px] text-white/88">{w}</span>
                <Trash2 className="h-3 w-3 text-emerald-300/50" strokeWidth={2} />
              </li>
            ))}
          </ul>
        </div>
      </div>
    </div>
  );
}

/* ─────────────────────────── 3. Predefined modes ────────────────────────── */
/* Rose-pink gradient + frosted-glass dock with three larger iOS-style mode
   tiles. INTERACTIVE: tiles are real role="radio" buttons inside a
   role="radiogroup". Click one → coral ring slides via Framer's layoutId.
   Default active: Message. */

type ModeId = "voice" | "message" | "email";

export function ModePickerVisual({ className }: VisualProps) {
  const reduce = useReducedMotion();
  const [activeId, setActiveId] = useState<ModeId>("message");

  const modes: Array<{
    id: ModeId;
    icon: typeof Mic;
    label: string;
  }> = [
    { id: "voice", icon: Mic, label: "Voice" },
    { id: "message", icon: MessageCircle, label: "Message" },
    { id: "email", icon: Mail, label: "Email" },
  ];

  return (
    <div
      className={className}
      style={{
        position: "absolute",
        inset: 0,
        background:
          "radial-gradient(ellipse 95% 90% at 50% 50%, #fb7185 0%, #be123c 55%, #4c0519 100%)",
      }}
    >
      {/* Top highlight */}
      <div
        aria-hidden
        className="absolute inset-0 pointer-events-none"
        style={{
          background:
            "radial-gradient(ellipse 60% 40% at 50% 0%, rgba(255,255,255,0.2), transparent 60%)",
        }}
      />

      <div className="absolute left-1/2 top-1/2 -translate-x-1/2 -translate-y-1/2 w-[92%] max-w-[300px]">
        <div
          role="radiogroup"
          aria-label="Mode"
          className="rounded-3xl px-3 py-3 flex items-end justify-around gap-2"
          style={{
            background: "rgba(255, 255, 255, 0.16)",
            backdropFilter: "blur(16px) saturate(150%)",
            WebkitBackdropFilter: "blur(16px) saturate(150%)",
            border: "1px solid rgba(255,255,255,0.22)",
            boxShadow: "0 10px 30px -8px rgba(0,0,0,0.4)",
          }}
        >
          {modes.map(({ id, icon: Icon, label }) => {
            const active = activeId === id;
            return (
              <button
                key={id}
                type="button"
                role="radio"
                aria-checked={active}
                aria-label={label}
                onClick={() => setActiveId(id)}
                className="flex flex-col items-center gap-1.5 flex-1 focus-visible:outline-none rounded-lg"
              >
                <div className="relative h-14 w-14">
                  {/* Tile face (always white) */}
                  <div
                    className="absolute inset-0 rounded-[14px] grid place-items-center"
                    style={{
                      background:
                        "linear-gradient(160deg, #ffffff 0%, #f4f4f5 100%)",
                      boxShadow:
                        "0 4px 10px -2px rgba(0,0,0,0.25), inset 0 1px 0 rgba(255,255,255,0.5)",
                    }}
                  >
                    <Icon
                      className={
                        active
                          ? "h-6 w-6 text-rose-500 transition-colors"
                          : "h-6 w-6 text-zinc-700 transition-colors"
                      }
                      strokeWidth={2}
                    />
                  </div>

                  {/* Active ring — animates between tiles via layoutId. */}
                  {active && (
                    <motion.div
                      layoutId="mode-active-ring"
                      aria-hidden
                      className="absolute inset-0 rounded-[14px] pointer-events-none"
                      style={{
                        boxShadow:
                          "0 0 0 2px #fb7185, 0 8px 16px -4px rgba(225, 29, 72, 0.5)",
                      }}
                      transition={
                        reduce
                          ? { duration: 0 }
                          : { type: "spring", stiffness: 360, damping: 32 }
                      }
                    />
                  )}
                </div>
                <span
                  className={
                    active
                      ? "text-[11px] font-medium text-white transition-colors"
                      : "text-[11px] text-white/70 transition-colors"
                  }
                >
                  {label}
                </span>
              </button>
            );
          })}
        </div>
      </div>
    </div>
  );
}

/* ─────────────────────────── 4. Multilingual support ─────────────────────── */
/* Deep indigo→midnight gradient + 6 big greeting words rendered as
   gradient-filled typography. No pill cards, no rotation — the words ARE
   the visual. */

export function MultilingualVisual({ className }: VisualProps) {
  const greetings: Array<{
    text: string;
    gradient: string;
    size: string;
    top: string;
    left: string;
  }> = [
    {
      text: "Hello",
      gradient: "linear-gradient(180deg, #ffffff 0%, #c4b5fd 100%)",
      size: "text-3xl md:text-4xl",
      top: "8%",
      left: "8%",
    },
    {
      text: "こんにちは",
      gradient: "linear-gradient(180deg, #ffffff 0%, #93c5fd 100%)",
      size: "text-2xl md:text-3xl",
      top: "14%",
      left: "55%",
    },
    {
      text: "Ciao",
      gradient: "linear-gradient(180deg, #ffffff 0%, #6ee7b7 100%)",
      size: "text-3xl md:text-4xl",
      top: "40%",
      left: "4%",
    },
    {
      text: "Привет",
      gradient: "linear-gradient(180deg, #ffffff 0%, #fda4af 100%)",
      size: "text-2xl md:text-3xl",
      top: "46%",
      left: "52%",
    },
    {
      text: "Olá",
      gradient: "linear-gradient(180deg, #ffffff 0%, #fcd34d 100%)",
      size: "text-3xl md:text-4xl",
      top: "30%",
      left: "30%",
    },
  ];

  return (
    <div
      aria-hidden
      className={className}
      style={{
        position: "absolute",
        inset: 0,
        background:
          "linear-gradient(160deg, #1e1b4b 0%, #0c0a3e 55%, #020617 100%)",
      }}
    >
      {/* Faint top highlight */}
      <div
        style={{
          position: "absolute",
          inset: 0,
          background:
            "radial-gradient(ellipse 60% 40% at 50% 0%, rgba(196, 181, 253, 0.16), transparent 60%)",
        }}
      />

      {greetings.map((g, i) => (
        <span
          key={i}
          className={`absolute font-display font-semibold whitespace-nowrap ${g.size}`}
          style={{
            top: g.top,
            left: g.left,
            background: g.gradient,
            WebkitBackgroundClip: "text",
            backgroundClip: "text",
            color: "transparent",
            textShadow: "0 0 24px rgba(165, 180, 252, 0.10)",
          }}
        >
          {g.text}
        </span>
      ))}
    </div>
  );
}

/* ─────────────────────────── 5. Clipboard integration ──────────────────── */
/* Deep green wallpaper with a faint icon-grid pattern + a single iMessage-
   style chat bubble + an iMessage-style compose input bar at the bottom. */

export function ClipboardChatVisual({ className }: VisualProps) {
  return (
    <div
      aria-hidden
      className={className}
      style={{
        position: "absolute",
        inset: 0,
        background:
          "linear-gradient(180deg, #064e3b 0%, #022c22 55%, #001a16 100%)",
      }}
    >
      {/* Faint icon-grid pattern (small generic app dots) */}
      <div
        style={{
          position: "absolute",
          inset: 0,
          opacity: 0.18,
          backgroundImage:
            "radial-gradient(circle at 16px 16px, rgba(110, 231, 183, 0.5) 1.5px, transparent 2px)",
          backgroundSize: "32px 32px",
        }}
      />

      {/* Chat bubble — centered */}
      <div className="absolute left-1/2 top-[44%] -translate-x-1/2 -translate-y-1/2 w-[78%] max-w-[260px]">
        <div
          className="rounded-2xl rounded-bl-md px-4 py-3"
          style={{
            background: "rgba(20, 70, 50, 0.78)",
            backdropFilter: "blur(8px)",
            WebkitBackdropFilter: "blur(8px)",
            border: "1px solid rgba(110, 231, 183, 0.18)",
            color: "rgba(236, 253, 245, 0.95)",
          }}
        >
          <p className="text-[12.5px] leading-snug">
            Hey! Wanna catch a movie this weekend? Let's grab the 7 pm show.
          </p>
        </div>
      </div>

      {/* Compose input bar at the bottom */}
      <div className="absolute left-1/2 bottom-4 -translate-x-1/2 w-[88%] max-w-[300px]">
        <div
          className="rounded-full pl-2 pr-1.5 py-1.5 flex items-center gap-2"
          style={{
            background: "rgba(255, 255, 255, 0.08)",
            backdropFilter: "blur(10px)",
            WebkitBackdropFilter: "blur(10px)",
            border: "1px solid rgba(110, 231, 183, 0.20)",
          }}
        >
          <span
            className="h-6 w-6 rounded-full grid place-items-center"
            style={{ background: "rgba(255,255,255,0.10)" }}
          >
            <Plus className="h-3.5 w-3.5 text-white/70" strokeWidth={2} />
          </span>
          <Smile className="h-4 w-4 text-white/60" strokeWidth={2} />
          <span className="flex-1 text-[12px] text-white/35">iMessage</span>
          <span
            className="h-6 w-6 rounded-full grid place-items-center"
            style={{ background: "rgba(74, 222, 128, 0.95)" }}
          >
            <Send className="h-3 w-3 text-emerald-950" strokeWidth={2.5} />
          </span>
        </div>
      </div>
    </div>
  );
}

/* ─────────────────────────── 6. Meeting assistant ────────────────────────── */
/* Photo-backed: laptop on a desk displaying a video conference + a small
   participant tile strip in the lower-left + a CSS-mock notes panel docked
   right. */

export function MeetingCallVisual({ className }: VisualProps) {
  return (
    <div
      aria-hidden
      className={className}
      style={{
        position: "absolute",
        inset: 0,
        background:
          "linear-gradient(180deg, #1c1917 0%, #292524 50%, #0c0a09 100%)",
        overflow: "hidden",
      }}
    >
      {/* Photo backdrop */}
      {/* eslint-disable-next-line @next/next/no-img-element */}
      <img
        src="/feature-meeting.jpg"
        alt=""
        loading="lazy"
        decoding="async"
        className="absolute inset-0 h-full w-full object-cover"
        onError={(e) => {
          (e.currentTarget as HTMLImageElement).style.display = "none";
        }}
      />

      {/* Dark overlay for foreground UI legibility */}
      <div
        style={{
          position: "absolute",
          inset: 0,
          background:
            "linear-gradient(180deg, rgba(0,0,0,0.15) 0%, rgba(0,0,0,0.55) 100%)",
        }}
      />

      {/* Participant tile strip — bottom-left */}
      <div className="absolute bottom-4 left-4 flex gap-1.5">
        {[
          { bg: "linear-gradient(135deg, #f59e0b, #b45309)", initial: "M" },
          { bg: "linear-gradient(135deg, #8b5cf6, #5b21b6)", initial: "S" },
          { bg: "linear-gradient(135deg, #06b6d4, #155e75)", initial: "J" },
          { bg: "linear-gradient(135deg, #ec4899, #9d174d)", initial: "L" },
        ].map((t, i) => (
          <div
            key={i}
            className="h-7 w-9 rounded grid place-items-center"
            style={{
              background: t.bg,
              border: "1px solid rgba(255,255,255,0.18)",
            }}
          >
            <span className="text-[10px] font-semibold text-white/95">
              {t.initial}
            </span>
          </div>
        ))}
      </div>

      {/* Notes panel — docked right */}
      <div className="absolute right-3 top-3 bottom-3 w-[38%] max-w-[150px]">
        <div
          className="h-full rounded-lg p-3 flex flex-col"
          style={{
            background: "rgba(10, 10, 11, 0.78)",
            backdropFilter: "blur(10px)",
            WebkitBackdropFilter: "blur(10px)",
            border: "1px solid rgba(255,255,255,0.10)",
          }}
        >
          <div className="text-[9px] uppercase tracking-[0.15em] text-cyan-300/85 font-semibold">
            Project timeline
          </div>
          <ul className="mt-2 space-y-1.5 flex-1">
            {[
              "85%",
              "62%",
              "78%",
              "55%",
              "70%",
              "48%",
              "82%",
              "60%",
            ].map((w, i) => (
              <li
                key={i}
                className="h-1.5 rounded-full"
                style={{
                  width: w,
                  background:
                    "linear-gradient(90deg, rgba(255,255,255,0.55) 0%, rgba(255,255,255,0.18) 100%)",
                }}
              />
            ))}
          </ul>
          <div className="mt-2 flex items-center gap-1">
            <span className="h-1.5 w-1.5 rounded-full bg-emerald-400" />
            <span className="text-[9px] text-white/60">Transcribing</span>
          </div>
        </div>
      </div>
    </div>
  );
}

/* ─────────────────────────── Registry / dispatcher ───────────────────────── */

export const VISUALS = {
  WifiToggleVisual,
  VocabPanelVisual,
  ModePickerVisual,
  MultilingualVisual,
  ClipboardChatVisual,
  MeetingCallVisual,
} as const;

export type VisualKey = keyof typeof VISUALS;
