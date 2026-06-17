"use client";

import { useReducedMotion } from "framer-motion";
import { Check, Cloud, Mic } from "lucide-react";
import { Section, SectionTitle, SectionLede } from "@/components/ui/Section";
import { whisperFlow } from "@/lib/content";
import { useTypewriter } from "@/lib/useTypewriter";

// All three columns share this 6s clock so the wave / cloud-pulse / typewriter
// feel like one choreographed event rather than three independent loops.
const LOOP_MS = 6000;
const TYPE_MS = 4800;

/* --------------------------------- Waveform --------------------------------- */

function Waveform({ animate }: { animate: boolean }) {
  return (
    <div
      className="flex items-end justify-center gap-[3px] h-16"
      aria-hidden
    >
      {Array.from({ length: 28 }).map((_, i) => (
        <span
          key={i}
          className={
            animate
              ? "w-[3px] rounded-full bg-accent animate-wfBar"
              : "w-[3px] rounded-full bg-accent"
          }
          style={{
            height: "100%",
            animationDelay: animate ? `${i * 0.06}s` : undefined,
            transform: animate ? undefined : "scaleY(0.6)",
            transformOrigin: "bottom",
          }}
        />
      ))}
    </div>
  );
}

/* ----------------------------------- Cloud ---------------------------------- */

function CloudPulse({ animate }: { animate: boolean }) {
  return (
    <div className="relative grid place-items-center h-24 w-24">
      {animate && (
        <>
          <span className="absolute inset-2 rounded-full bg-accent/25 animate-cloudPulseA" />
          <span className="absolute inset-2 rounded-full bg-accent/25 animate-cloudPulseB" />
        </>
      )}
      <div className="relative grid place-items-center h-20 w-20 rounded-full bg-ink-700 border border-ink-50/8 shadow-[inset_0_1px_0_rgba(255,255,255,0.06),0_8px_24px_-12px_rgba(0,0,0,0.6)]">
        <Cloud className="h-9 w-9 text-accent" strokeWidth={1.5} />
      </div>
    </div>
  );
}

/* --------------------------------- Section ---------------------------------- */

export function WhisperFlow() {
  const reduce = useReducedMotion() ?? false;
  const animate = !reduce;
  const chars = useTypewriter({
    text: whisperFlow.cleaned,
    loopMs: LOOP_MS,
    typeMs: TYPE_MS,
    enabled: animate,
  });

  return (
    <Section id="flow">
      <div className="max-w-3xl">
        <p className="text-xs uppercase tracking-[0.2em] text-accent mb-4">
          {whisperFlow.eyebrow}
        </p>
        <SectionTitle>{whisperFlow.title}</SectionTitle>
        <SectionLede>{whisperFlow.body}</SectionLede>
      </div>

      <div className="mt-14 rounded-2xl bg-ink-800 hairline p-6 md:p-10">
        <div className="grid md:grid-cols-3 gap-10 md:gap-6 items-start">
          {/* Left — voice in */}
          <div className="flex flex-col gap-4">
            <div className="flex items-center gap-2">
              <span className="grid h-6 w-6 place-items-center rounded-full bg-red-500/15">
                <Mic className="h-3.5 w-3.5 text-red-400" strokeWidth={2} />
              </span>
              <span className="text-xs uppercase tracking-[0.15em] text-ink-300">
                You
              </span>
            </div>
            <Waveform animate={animate} />
            <p className="text-sm text-ink-200 leading-relaxed font-mono">
              &ldquo;{whisperFlow.spoken}&rdquo;
            </p>
          </div>

          {/* Middle — cloud processing */}
          <div className="flex flex-col items-center justify-center gap-4 py-4">
            <CloudPulse animate={animate} />
            <p className="text-xs text-ink-300 font-mono">
              {whisperFlow.cloudLabel} · 0.4s
            </p>
          </div>

          {/* Right — clean text out */}
          <div className="flex flex-col gap-4">
            <div className="flex items-center gap-2">
              <span className="grid h-6 w-6 place-items-center rounded-full bg-accent/15">
                <Check className="h-3.5 w-3.5 text-accent" strokeWidth={2.25} />
              </span>
              <span className="text-xs uppercase tracking-[0.15em] text-ink-300">
                Airnote
              </span>
            </div>
            <p className="text-base text-ink-50 leading-relaxed min-h-[5rem]">
              {whisperFlow.cleaned.slice(0, chars)}
              {animate && (
                <span className="inline-block w-[1ch] animate-caret">|</span>
              )}
            </p>
            <div className="inline-flex items-center gap-1.5 text-xs text-ink-300">
              <Check className="h-3 w-3 text-green-400" strokeWidth={2.5} />
              {whisperFlow.badge}
            </div>
          </div>
        </div>

        {/* Shared progress bar — the visual heartbeat tying the columns together */}
        <div className="mt-10 relative h-[2px] rounded-full bg-ink-50/5 overflow-hidden">
          {animate ? (
            <span className="absolute inset-y-0 w-1/3 bg-gradient-to-r from-transparent via-accent to-transparent animate-flowProgress" />
          ) : (
            <span className="absolute inset-y-0 left-0 right-0 bg-accent/20" />
          )}
        </div>
      </div>
    </Section>
  );
}
