"use client";

import { useCallback, useEffect, useState } from "react";
import { motion, useReducedMotion } from "framer-motion";
import { Play } from "lucide-react";
import { heroDemo } from "@/lib/content";
import { MockNotesWindow } from "@/components/demo/MockNotesWindow";
import { MockSlackWindow } from "@/components/demo/MockSlackWindow";
import { MockCursorWindow } from "@/components/demo/MockCursorWindow";
import { MockDock } from "@/components/demo/MockDock";
import { ListeningPill } from "@/components/demo/ListeningPill";
import { cn } from "@/lib/cn";

/* ---------------------------- Phase + timing const --------------------------- */

type Phase = "ready" | "playing" | "blurred";

const PLAY_END_MS = 10500;
const HOLD_START_MS = 8500;
const NOTES_TYPE_START = 2700;
const NOTES_TYPE_END = 6000;

/** Cubic ease-out — physical "settle" feel for window entrances. */
function easeOutQuart(t: number): number {
  return 1 - Math.pow(1 - t, 4);
}

/** Eased 0..1 ramp clamped between [start, end]. */
function eased(elapsed: number, start: number, end: number): number {
  if (elapsed <= start) return 0;
  if (elapsed >= end) return 1;
  return easeOutQuart((elapsed - start) / (end - start));
}

/** Linear 0..1 ramp — used for the small ambient elements (pill, dock). */
function linear(elapsed: number, start: number, end: number): number {
  if (elapsed <= start) return 0;
  if (elapsed >= end) return 1;
  return (elapsed - start) / (end - start);
}

/* ------------------------------ Play / Replay --------------------------- */

function PlayPill({
  label,
  onClick,
}: {
  label: string;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        "group inline-flex items-center gap-2.5 rounded-full px-5 py-2.5",
        "bg-white/25 hover:bg-white/35 active:translate-y-px",
        "backdrop-blur-md border border-white/40",
        "text-sm font-medium text-white",
        "transition-colors",
        "shadow-[0_10px_28px_-10px_rgba(0,0,0,0.45)]",
      )}
    >
      <span className="grid h-7 w-7 place-items-center rounded-full bg-white/40 group-hover:bg-white/55 transition-colors">
        <Play className="ml-0.5 h-3.5 w-3.5" strokeWidth={2.5} />
      </span>
      {label}
    </button>
  );
}

/* ---------------------------------- Demo ---------------------------------- */

export function HeroDemo() {
  const reduce = useReducedMotion() ?? false;
  const [phase, setPhase] = useState<Phase>("ready");
  const [elapsed, setElapsed] = useState(0);

  // Drive the elapsed clock while the demo is "playing".
  useEffect(() => {
    if (phase !== "playing") return;
    if (reduce) {
      setElapsed(PLAY_END_MS);
      setPhase("blurred");
      return;
    }
    let raf = 0;
    const start = performance.now();
    const tick = (now: number) => {
      const e = now - start;
      if (e >= PLAY_END_MS) {
        setElapsed(PLAY_END_MS);
        setPhase("blurred");
        return;
      }
      setElapsed(e);
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [phase, reduce]);

  const handlePlay = useCallback(() => {
    setElapsed(0);
    setPhase("playing");
  }, []);

  // When reduce-motion is on, force every animation to its final value.
  const e = reduce ? PLAY_END_MS : elapsed;
  const showScene = phase !== "ready";

  // Ambient elements use linear (small, fast).
  const pillT = linear(e, 0, 600);
  const dockT = linear(e, 200, 800);

  // Windows use eased (physical settle).
  const notesT = eased(e, 1000, 2100);
  const slackT = eased(e, 5500, 6700);
  const cursorT = eased(e, 7200, 8500);

  // Hold-phase bob — small floating motion once all windows are in place.
  // Different phase offset per window so they don't sync.
  const bobActive = !reduce && e >= HOLD_START_MS;
  const bob = (phaseOffset: number) =>
    bobActive ? Math.sin((e - HOLD_START_MS) / 600 + phaseOffset) * 1.5 : 0;

  // Notes typewriter — derived from the same shared clock.
  const notesBody = heroDemo.notes.body;
  const bodyChars = reduce
    ? notesBody.length
    : e < NOTES_TYPE_START
      ? 0
      : e >= NOTES_TYPE_END
        ? notesBody.length
        : Math.floor(
            ((e - NOTES_TYPE_START) / (NOTES_TYPE_END - NOTES_TYPE_START)) *
              notesBody.length,
          );

  // Headless: the parent (Hero.tsx's morphing stage or the reduced-motion
  // fallback in Hero.tsx) provides the frame + backdrop. We just render the
  // three phase-conditional layers as absolute overlays.
  return (
    <>
      {/* Pre-roll state */}
      {phase === "ready" && (
        <div className="absolute inset-0 flex flex-col items-center justify-center gap-4">
          <p
            className="text-lg md:text-xl font-semibold text-white"
            style={{ textShadow: "0 1px 20px rgba(0,0,0,0.45)" }}
          >
            {heroDemo.prePromptTitle}
          </p>
          <PlayPill label={heroDemo.playLabel} onClick={handlePlay} />
        </div>
      )}

      {/* Animated scene (playing + blurred phases share this) */}
      {showScene && (
        <motion.div
          aria-hidden={phase === "blurred"}
          className="absolute inset-0"
          animate={{
            filter:
              phase === "blurred"
                ? "blur(12px) brightness(0.85) saturate(0.9)"
                : "blur(0px) brightness(1) saturate(1)",
          }}
          transition={{ duration: 0.6, ease: [0.22, 1, 0.36, 1] }}
        >
          {/* Listening pill — top center */}
          <div
            className="absolute left-1/2"
            style={{
              top: 24,
              opacity: pillT,
              transform: `translateX(-50%) translateY(${(1 - pillT) * -36}px)`,
            }}
          >
            <ListeningPill animate={!reduce} />
          </div>

          {/* Slack window — middle-left, behind */}
          <div
            className="absolute"
            style={{
              left: "3%",
              top: "22%",
              width: "44%",
              opacity: slackT,
              transform: `translateX(${(1 - slackT) * -50}px) translateY(${bob(1.2)}px) scale(${0.96 + slackT * 0.04})`,
              zIndex: 8,
            }}
          >
            <MockSlackWindow className="w-full" />
          </div>

          {/* Notes window — top-right */}
          <div
            className="absolute"
            style={{
              right: "5%",
              top: "10%",
              width: "34%",
              opacity: notesT,
              transform: `translateX(${(1 - notesT) * 50}px) translateY(${bob(0)}px) scale(${0.96 + notesT * 0.04})`,
              zIndex: 10,
            }}
          >
            <MockNotesWindow
              bodyChars={bodyChars}
              showCaret={!reduce && e >= 2000 && e < NOTES_TYPE_END + 500}
            />
          </div>

          {/* Cursor window — bottom-center, foregrounded */}
          <div
            className="absolute"
            style={{
              left: "18%",
              top: "35%",
              width: "65%",
              opacity: cursorT,
              transform: `translateY(${(1 - cursorT) * 40 + bob(2.4)}px) scale(${0.94 + cursorT * 0.06})`,
              zIndex: 15,
            }}
          >
            <MockCursorWindow className="w-full" />
          </div>

          {/* Dock — bottom center */}
          <div
            className="absolute bottom-3 left-1/2"
            style={{
              opacity: dockT,
              transform: `translateX(-50%) translateY(${(1 - dockT) * 24}px)`,
              zIndex: 5,
            }}
          >
            <MockDock />
          </div>
        </motion.div>
      )}

      {/* Replay overlay — appears when blurred */}
      {phase === "blurred" && (
        <motion.div
          initial={{ opacity: 0, scale: 0.95 }}
          animate={{ opacity: 1, scale: 1 }}
          transition={{ duration: 0.4, delay: 0.2, ease: [0.22, 1, 0.36, 1] }}
          className="absolute inset-0 z-30 flex items-center justify-center"
        >
          <PlayPill label={heroDemo.replayLabel} onClick={handlePlay} />
        </motion.div>
      )}
    </>
  );
}
