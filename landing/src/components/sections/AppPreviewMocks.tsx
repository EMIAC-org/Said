"use client";

/* ========================================================================== */
/*  AppPreviewMocks — CSS-rendered settings panes shown inside the AppPreview */
/*  macOS window. Three views: Models, Enterprise, About. Faithful to the     */
/*  user's Airnote v2.3.2 screenshots: same model names, same connected org,  */
/*  same version + tech-stack tagline + diagnostics + Beta channel + update.  */
/*                                                                            */
/*  Each view fills its parent (`absolute inset-0`) and uses the same brand   */
/*  chrome as the rest of the project — periwinkle accent (`--accent`),       */
/*  glass cards on `bg-ink-900/70`, hairline borders, lucide-react icons.     */
/*                                                                            */
/*  Interactivity (iteration 22):                                             */
/*    Models     — Fast / Smart radio cards toggle on click (animated         */
/*                 layoutId ring slides between them).                        */
/*    Enterprise — Disconnect button has hover state.                         */
/*    About      — Diagnostics toggle flips on click (thumb slides);          */
/*                 Stable / Beta segmented control swaps (layoutId pill).    */
/*  All buttons across all three views get cursor-pointer + hover bg shift.   */
/* ========================================================================== */

import { useState } from "react";
import { motion, useReducedMotion } from "framer-motion";
import {
  Bug,
  Check,
  Download,
  GitBranch,
  Info,
  Sparkles,
  Wifi,
  Zap,
  type LucideIcon,
} from "lucide-react";
import { cn } from "@/lib/cn";

const ACCENT = "#a5b4fc";       // periwinkle (matches --accent)
const ACCENT_RGB = "165, 180, 252";

/* ─────────────────────────── Shared sub-components ───────────────────────── */

function EyebrowRow({ children }: { children: React.ReactNode }) {
  return (
    <div className="flex items-center gap-2 mb-3">
      <span
        aria-hidden
        className="h-1.5 w-1.5 rounded-full"
        style={{ background: ACCENT }}
      />
      <span className="text-[10px] uppercase tracking-[0.18em] text-ink-300 font-semibold">
        {children}
      </span>
    </div>
  );
}

function IconChip({
  Icon,
  tone = "neutral",
}: {
  Icon: LucideIcon;
  tone?: "neutral" | "accent" | "amber" | "green" | "violet";
}) {
  const styles = {
    neutral: { bg: "rgba(255,255,255,0.06)", color: "rgba(229,229,235,0.85)" },
    accent:  { bg: "rgba(165,180,252,0.14)", color: ACCENT },
    amber:   { bg: "rgba(251,191,36,0.14)",  color: "#fbbf24" },
    green:   { bg: "rgba(74,222,128,0.14)",  color: "#4ade80" },
    violet:  { bg: "rgba(167,139,250,0.16)", color: "#c4b5fd" },
  }[tone];
  return (
    <span
      className="grid place-items-center h-9 w-9 rounded-lg shrink-0"
      style={{ background: styles.bg, color: styles.color }}
    >
      <Icon className="h-4 w-4" strokeWidth={2} />
    </span>
  );
}

function GlassCard({
  children,
  className = "",
}: {
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <div
      className={cn(
        "rounded-2xl p-4 md:p-5",
        className,
      )}
      style={{
        background: "rgba(22, 22, 28, 0.7)",
        border: "1px solid rgba(255,255,255,0.06)",
      }}
    >
      {children}
    </div>
  );
}

function Divider() {
  return <div className="h-px my-3" style={{ background: "rgba(255,255,255,0.06)" }} />;
}

/* ─────────────────────────────── ModelsView ──────────────────────────────── */
/* Two stacked sections: DICTATION MODEL with Fast/Smart radio (Smart picked),
   then CHATGPT with the connected status. */

type ModelChoice = "fast" | "smart";

export function ModelsView() {
  const reduce = useReducedMotion();
  const [choice, setChoice] = useState<ModelChoice>("smart");

  const options: Array<{
    id: ModelChoice;
    label: string;
    sub: string;
    Icon: LucideIcon;
  }> = [
    { id: "fast",  label: "Fast",  sub: "Llama 3.1 8B - lowest latency",      Icon: Zap },
    { id: "smart", label: "Smart", sub: "Llama 4 Scout - better for complex sentences", Icon: Sparkles },
  ];

  return (
    <div className="absolute inset-0 overflow-y-auto no-scrollbar px-6 py-5 md:px-7 md:py-6 flex flex-col gap-6">
      {/* DICTATION MODEL */}
      <section>
        <EyebrowRow>DICTATION MODEL</EyebrowRow>
        <GlassCard>
          <div className="flex items-start gap-3">
            <IconChip Icon={Zap} tone="accent" />
            <div className="min-w-0 flex-1">
              <div className="text-[14px] font-semibold text-ink-50">
                Normal voice polish
              </div>
              <p className="mt-0.5 text-[12.5px] text-ink-300 leading-snug">
                Groq is the fixed provider. Choose speed vs quality for
                regular hotkey dictation.
              </p>
            </div>
          </div>

          {/* Fast / Smart radio row — clickable, with layoutId ring */}
          <div
            role="radiogroup"
            aria-label="Dictation model"
            className="mt-4 grid grid-cols-2 gap-2"
          >
            {options.map(({ id, label, sub, Icon }) => {
              const active = choice === id;
              return (
                <button
                  key={id}
                  type="button"
                  role="radio"
                  aria-checked={active}
                  onClick={() => setChoice(id)}
                  className={cn(
                    "relative text-left rounded-xl px-3.5 py-3 cursor-pointer",
                    "transition-colors duration-200",
                    "focus-visible:outline-none",
                    !active && "hover:bg-white/[0.06]",
                  )}
                  style={{
                    background: active
                      ? `rgba(${ACCENT_RGB}, 0.08)`
                      : "rgba(255,255,255,0.03)",
                    border: active
                      ? `1px solid rgba(${ACCENT_RGB}, 0.0)` // hidden — layoutId ring carries it
                      : "1px solid rgba(255,255,255,0.06)",
                  }}
                >
                  {/* Active outline — slides between cards via layoutId. */}
                  {active && (
                    <motion.span
                      layoutId="models-active-ring"
                      aria-hidden
                      className="absolute inset-0 rounded-xl pointer-events-none"
                      style={{
                        border: `1px solid rgba(${ACCENT_RGB}, 0.55)`,
                        boxShadow: `0 0 0 3px rgba(${ACCENT_RGB}, 0.10)`,
                      }}
                      transition={
                        reduce
                          ? { duration: 0 }
                          : { type: "spring", stiffness: 380, damping: 32 }
                      }
                    />
                  )}
                  {active && (
                    <Check
                      className="absolute top-2.5 right-2.5 h-3.5 w-3.5"
                      strokeWidth={2.5}
                      style={{ color: ACCENT }}
                    />
                  )}
                  <div
                    className="flex items-center gap-1.5 text-[13px] font-medium transition-colors"
                    style={{ color: active ? ACCENT : "rgb(228 228 231)" }}
                  >
                    <Icon className="h-3.5 w-3.5" strokeWidth={2} />
                    {label}
                  </div>
                  <div
                    className={cn(
                      "mt-0.5 text-[11.5px] transition-colors",
                      active ? "text-ink-200" : "text-ink-300",
                    )}
                  >
                    {sub}
                  </div>
                </button>
              );
            })}
          </div>
        </GlassCard>
      </section>

      {/* CHATGPT */}
      <section>
        <EyebrowRow>CHATGPT</EyebrowRow>
        <GlassCard>
          <div className="flex items-start gap-3">
            <IconChip Icon={Sparkles} tone="accent" />
            <div className="min-w-0 flex-1">
              <div className="flex items-center justify-between gap-2">
                <div className="text-[14px] font-semibold text-ink-50">
                  ChatGPT Connected
                </div>
                <div className="flex items-center gap-2 shrink-0">
                  <span
                    className="inline-flex items-center rounded-full px-2 py-0.5 text-[10.5px] font-semibold"
                    style={{
                      background: "rgba(74,222,128,0.15)",
                      color: "#4ade80",
                    }}
                  >
                    Connected
                  </span>
                  <button
                    type="button"
                    className={cn(
                      "rounded-md px-2 py-1 text-[11.5px] cursor-pointer",
                      "text-ink-300 hover:text-ink-50 hover:bg-white/[0.06]",
                      "focus-visible:outline-none transition-colors",
                    )}
                  >
                    Disconnect
                  </button>
                </div>
              </div>
              <p className="mt-0.5 text-[12.5px] text-ink-300 leading-snug">
                Used for shortcut transforms and repair / refine ·{" "}
                <span className="text-ink-200">expires 9 Jun 2026</span>
              </p>
            </div>
          </div>
        </GlassCard>
      </section>
    </div>
  );
}

/* ──────────────────────────── EnterpriseView ─────────────────────────────── */
/* Single section: ENTERPRISE eyebrow + one card containing the connected
   organization header row + a divider + the Server row with URL + Disconnect. */

export function EnterpriseView() {
  return (
    <div className="absolute inset-0 overflow-y-auto no-scrollbar px-6 py-5 md:px-7 md:py-6 flex flex-col gap-6">
      <section>
        <EyebrowRow>ENTERPRISE</EyebrowRow>
        <GlassCard>
          {/* Organization header row */}
          <div className="flex items-center gap-3">
            <div
              aria-hidden
              className="grid place-items-center h-9 w-9 rounded-full text-[13px] font-semibold text-white shrink-0"
              style={{
                background:
                  "linear-gradient(135deg, #f59e0b 0%, #d97706 60%, #92400e 100%)",
                boxShadow: "inset 0 1px 0 rgba(255,255,255,0.2)",
              }}
            >
              A
            </div>
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-2">
                <div className="text-[14px] font-semibold text-ink-50">
                  emiactech (auto)
                </div>
                <span
                  className="inline-flex items-center rounded-full px-2 py-0.5 text-[10.5px] font-semibold"
                  style={{
                    background: "rgba(74,222,128,0.15)",
                    color: "#4ade80",
                  }}
                >
                  Connected
                </span>
              </div>
              <div className="mt-0.5 text-[12.5px] text-ink-300">
                Abhishek Verma · abhishek@emiactech.com
              </div>
            </div>
          </div>

          <Divider />

          {/* Server row */}
          <div className="flex items-center gap-3">
            <IconChip Icon={Wifi} tone="accent" />
            <div className="min-w-0 flex-1">
              <div className="text-[13px] font-semibold text-ink-50">
                Server
              </div>
              <div className="mt-0.5 text-[12px] font-mono text-ink-300">
                https://airnote.emiactech.com
              </div>
            </div>
            <button
              type="button"
              className={cn(
                "rounded-md px-3 py-1.5 text-[12px] font-medium shrink-0 cursor-pointer",
                "transition-all duration-200",
                "hover:bg-[rgba(239,68,68,0.20)] hover:border-[rgba(239,68,68,0.38)]",
                "focus-visible:outline-none",
              )}
              style={{
                background: "rgba(239, 68, 68, 0.10)",
                color: "#f87171",
                border: "1px solid rgba(239, 68, 68, 0.22)",
              }}
            >
              Disconnect
            </button>
          </div>
        </GlassCard>
      </section>
    </div>
  );
}

/* ────────────────────────────── AboutView ────────────────────────────────── */
/* Single card containing four stacked rows: header (v2.3.2), diagnostics
   toggle, update channel, software update. */

type Channel = "stable" | "beta";

export function AboutView() {
  const reduce = useReducedMotion();
  const [diagnostics, setDiagnostics] = useState(true);
  const [channel, setChannel] = useState<Channel>("beta");

  return (
    <div className="absolute inset-0 overflow-y-auto no-scrollbar px-6 py-5 md:px-7 md:py-6 flex flex-col gap-6">
      <section>
        <EyebrowRow>ABOUT</EyebrowRow>
        <GlassCard>
          {/* Header row */}
          <div className="flex items-start gap-3">
            <IconChip Icon={Info} tone="accent" />
            <div className="min-w-0 flex-1">
              <div className="text-[14px] font-semibold text-ink-50">
                AirNote v2.3.2
              </div>
              <p className="mt-0.5 text-[12.5px] text-ink-300 leading-snug">
                Voice Polish Studio · Local-first · Tauri + Rust + React
              </p>
            </div>
          </div>

          <Divider />

          {/* Diagnostics row + INTERACTIVE toggle */}
          <div className="flex items-start gap-3">
            <IconChip Icon={Bug} tone="violet" />
            <div className="min-w-0 flex-1">
              <div className="text-[13px] font-semibold text-ink-50">
                Send anonymous diagnostics
              </div>
              <p className="mt-0.5 text-[12px] text-ink-300 leading-snug">
                {diagnostics ? "On" : "Off"} — anonymous crash reports +
                error logs. No content, no audio, no API keys. Restart to
                apply changes.
              </p>
            </div>
            <button
              type="button"
              role="switch"
              aria-checked={diagnostics}
              aria-label="Send anonymous diagnostics"
              onClick={() => setDiagnostics((v) => !v)}
              className="relative h-6 w-11 rounded-full shrink-0 cursor-pointer focus-visible:outline-none transition-colors duration-200"
              style={{
                background: diagnostics
                  ? `rgba(${ACCENT_RGB}, 0.85)`
                  : "rgba(255,255,255,0.10)",
                boxShadow: `inset 0 1px 0 rgba(255,255,255,0.18)`,
              }}
            >
              <motion.span
                aria-hidden
                className="absolute top-0.5 h-5 w-5 rounded-full bg-white"
                style={{
                  boxShadow: "0 1px 3px rgba(0,0,0,0.3)",
                }}
                animate={{
                  left: diagnostics ? "calc(100% - 22px)" : "2px",
                }}
                transition={
                  reduce
                    ? { duration: 0 }
                    : { type: "spring", stiffness: 480, damping: 32 }
                }
              />
            </button>
          </div>

          <Divider />

          {/* Update channel row + INTERACTIVE Stable/Beta segmented control */}
          <div className="flex items-start gap-3">
            <IconChip Icon={GitBranch} tone="accent" />
            <div className="min-w-0 flex-1">
              <div className="text-[13px] font-semibold text-ink-50">
                Update channel
              </div>
              <p className="mt-0.5 text-[12px] text-ink-300 leading-snug">
                {channel === "beta"
                  ? "Beta — preview builds when available. Pref stored; runtime endpoint switch ships in v3.x."
                  : "Stable — production builds only. Updates auto-apply on app restart."}
              </p>
            </div>
            <div
              role="radiogroup"
              aria-label="Update channel"
              className="flex items-center gap-0.5 rounded-md p-0.5 shrink-0"
              style={{
                background: "rgba(255,255,255,0.04)",
                border: "1px solid rgba(255,255,255,0.06)",
              }}
            >
              {(["stable", "beta"] as const).map((c) => {
                const active = channel === c;
                return (
                  <button
                    key={c}
                    type="button"
                    role="radio"
                    aria-checked={active}
                    onClick={() => setChannel(c)}
                    className={cn(
                      "relative rounded px-2.5 py-1 text-[11.5px] cursor-pointer",
                      "transition-colors duration-200",
                      "focus-visible:outline-none",
                      active ? "font-medium" : "text-ink-300 hover:text-ink-50",
                    )}
                    style={
                      active
                        ? { color: ACCENT }
                        : undefined
                    }
                  >
                    {active && (
                      <motion.span
                        layoutId="about-channel-pill"
                        aria-hidden
                        className="absolute inset-0 rounded"
                        style={{ background: `rgba(${ACCENT_RGB}, 0.18)` }}
                        transition={
                          reduce
                            ? { duration: 0 }
                            : { type: "spring", stiffness: 480, damping: 32 }
                        }
                      />
                    )}
                    <span className="relative z-10 capitalize">{c}</span>
                  </button>
                );
              })}
            </div>
          </div>

          <Divider />

          {/* Software Update row + Check button (hover state) */}
          <div className="flex items-start gap-3">
            <IconChip Icon={Download} tone="accent" />
            <div className="min-w-0 flex-1">
              <div className="text-[13px] font-semibold text-ink-50">
                Software Update
              </div>
              <p className="mt-0.5 text-[12px] text-ink-300 leading-snug">
                Check for available updates
              </p>
            </div>
            <button
              type="button"
              className={cn(
                "rounded-md px-3 py-1.5 text-[12px] font-medium text-ink-50 shrink-0 cursor-pointer",
                "transition-colors duration-200",
                "hover:bg-white/[0.10] hover:border-white/[0.18]",
                "focus-visible:outline-none",
              )}
              style={{
                background: "rgba(255,255,255,0.05)",
                border: "1px solid rgba(255,255,255,0.10)",
              }}
            >
              Check
            </button>
          </div>
        </GlassCard>
      </section>
    </div>
  );
}

/* ─────────────────────────── Dispatch map ────────────────────────────────── */
/* Consumed by SettingsPreview.tsx — picks the right view component based on
   the active tab id. */

export const APP_PREVIEW_VIEWS = {
  models: ModelsView,
  enterprise: EnterpriseView,
  about: AboutView,
} as const;

export type AppPreviewViewKey = keyof typeof APP_PREVIEW_VIEWS;
