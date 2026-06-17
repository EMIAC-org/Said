"use client";

import {
  useEffect,
  useRef,
  useState,
  type CSSProperties,
} from "react";
import * as Tabs from "@radix-ui/react-tabs";
import { motion, useReducedMotion } from "framer-motion";
import { Section } from "@/components/ui/Section";
import { toneSwitcher } from "@/lib/content";
import { cn } from "@/lib/cn";

/* ============================================================================
   Pacing constants — tune the feel here.
   ========================================================================= */
const TYPE_MS_PER_CHAR_OUT = 30;   // outgoing messages — user dictating
const TYPE_MS_PER_CHAR_IN = 24;    // incoming messages — snappier
const HOLD_BETWEEN_MS = 700;       // pause after each message before next types
const HOLD_END_OF_TONE_MS = 2400;  // longer pause after last message, then advance
const REDUCED_MOTION_STEP_MS = 1600; // when prefers-reduced-motion, fixed cadence

/* ────────────────────────── Outgoing bubble (right) ──────────────────────── */
/* iMessage-influenced sent bubble, coloured per the active tone. */

function OutgoingBubble({
  children,
  color,
  colorRgb,
}: {
  children: React.ReactNode;
  color: string;
  colorRgb: string;
}) {
  return (
    <div
      className="rounded-3xl rounded-br-md px-4 py-2.5 text-white text-[15px] leading-snug max-w-[22em]"
      style={{
        background: color,
        boxShadow: `0 1px 0 rgba(255,255,255,0.18) inset, 0 8px 18px -8px rgba(${colorRgb}, 0.45)`,
        transition:
          "background 350ms cubic-bezier(0.22, 1, 0.36, 1), box-shadow 350ms cubic-bezier(0.22, 1, 0.36, 1)",
      }}
    >
      {children}
    </div>
  );
}

/* ────────────────────────── Incoming bubble (left) ───────────────────────── */
/* Dark glass bubble for Aarav's replies. Stays neutral across tones since
   it represents the other side of the conversation, not the user's tone. */

function IncomingBubble({ children }: { children: React.ReactNode }) {
  return (
    <div
      className="rounded-3xl rounded-bl-md px-4 py-2.5 text-ink-50 text-[15px] leading-snug max-w-[22em]"
      style={{
        background: "rgba(255,255,255,0.06)",
        border: "1px solid rgba(255,255,255,0.10)",
        backdropFilter: "blur(8px)",
        WebkitBackdropFilter: "blur(8px)",
      }}
    >
      {children}
    </div>
  );
}

/* ────────────────────────────── Caret cursor ─────────────────────────────── */

function Caret() {
  return <span className="inline-block w-[1ch] animate-caret">|</span>;
}

/* ────────────────────────────── Chat preview ─────────────────────────────── */
/* Plays the active tone's conversation as a sequence of bubbles. Each
   message types in via an inline rAF loop. Once the last message
   completes, calls onConversationComplete so the parent can advance to
   the next tone. Auto-scrolls to the bottom on each new message. */

type Message = { side: "out" | "in"; text: string };

function ChatPreview({
  messages,
  color,
  colorRgb,
  animate,
  onConversationComplete,
}: {
  messages: readonly Message[];
  color: string;
  colorRgb: string;
  animate: boolean;
  onConversationComplete: () => void;
}) {
  const [msgIndex, setMsgIndex] = useState(0);
  // chars revealed for the CURRENT (typing) message. 0 → still typing; full
  // length → done.
  const [chars, setChars] = useState(0);

  const current = messages[msgIndex];
  // Keep onComplete in a ref so the effect that schedules advance reads the
  // latest closure without re-running.
  const onCompleteRef = useRef(onConversationComplete);
  useEffect(() => {
    onCompleteRef.current = onConversationComplete;
  });

  /* ── Effect 1: drive the typewriter for the current message. ─────────── */
  useEffect(() => {
    if (!current) return;
    if (!animate) {
      // Reduced motion: render full text immediately.
      setChars(current.text.length);
      return;
    }

    setChars(0); // reset on message advance
    const total = current.text.length;
    const typeMs =
      total *
      (current.side === "out" ? TYPE_MS_PER_CHAR_OUT : TYPE_MS_PER_CHAR_IN);
    const start = performance.now();
    let raf = 0;

    const tick = (now: number) => {
      const elapsed = now - start;
      const c = Math.min(total, Math.floor((elapsed / typeMs) * total));
      setChars(c);
      if (c < total) {
        raf = requestAnimationFrame(tick);
      }
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [msgIndex, animate, current]);

  /* ── Effect 2: once the current message is fully typed, hold then advance.
       For the last message, hold longer and call onConversationComplete so
       the parent can transition to the next tone. ─────────────────────── */
  const isLast = msgIndex === messages.length - 1;
  const isComplete = !!current && chars >= current.text.length;
  useEffect(() => {
    if (!isComplete) return;
    const delay = animate
      ? isLast
        ? HOLD_END_OF_TONE_MS
        : HOLD_BETWEEN_MS
      : REDUCED_MOTION_STEP_MS;
    const timer = setTimeout(() => {
      if (isLast) {
        onCompleteRef.current();
      } else {
        setMsgIndex((i) => i + 1);
      }
    }, delay);
    return () => clearTimeout(timer);
  }, [isComplete, isLast, animate]);

  /* ── Effect 3: auto-scroll to the newest message. ────────────────────── */
  const containerRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    el.scrollTo({
      top: el.scrollHeight,
      behavior: animate ? "smooth" : "auto",
    });
  }, [msgIndex, animate]);

  /* ── Render ──────────────────────────────────────────────────────────── */
  return (
    <div
      ref={containerRef}
      className="rounded-3xl p-5 md:p-6 no-scrollbar flex flex-col gap-3.5"
      style={{
        background: `rgba(10, 10, 11, 0.55)`,
        backdropFilter: "blur(12px)",
        WebkitBackdropFilter: "blur(12px)",
        border: `1px solid rgba(${colorRgb}, 0.18)`,
        boxShadow: `0 30px 60px -20px rgba(${colorRgb}, 0.18)`,
        height: 460,
        maxHeight: 460,
        overflowY: "auto",
        transition:
          "border-color 350ms cubic-bezier(0.22, 1, 0.36, 1), box-shadow 350ms cubic-bezier(0.22, 1, 0.36, 1)",
      }}
    >
      {messages.slice(0, msgIndex + 1).map((m, i) => {
        const isCurrent = i === msgIndex;
        const display = isCurrent ? m.text.slice(0, chars) : m.text;
        const showCaret = isCurrent && animate && chars < m.text.length;
        return (
          <motion.div
            key={i}
            initial={animate ? { opacity: 0, y: 8 } : false}
            animate={{ opacity: 1, y: 0 }}
            transition={{ duration: 0.3, ease: [0.22, 1, 0.36, 1] }}
            className={
              m.side === "out" ? "flex justify-end" : "flex justify-start"
            }
          >
            {m.side === "out" ? (
              <OutgoingBubble color={color} colorRgb={colorRgb}>
                {display}
                {showCaret && <Caret />}
              </OutgoingBubble>
            ) : (
              <IncomingBubble>
                {display}
                {showCaret && <Caret />}
              </IncomingBubble>
            )}
          </motion.div>
        );
      })}
    </div>
  );
}

/* ──────────────────────────────── Section ────────────────────────────────── */

export function ToneSwitcher() {
  const reduce = useReducedMotion();
  const animate = !reduce;

  const [active, setActive] = useState<string>(toneSwitcher.modes[0].id);
  // Once the user clicks any tab, stop auto-advancing — never override the
  // explicit choice.
  const userInteractedRef = useRef(false);

  const handleTabChange = (next: string) => {
    userInteractedRef.current = true;
    setActive(next);
  };

  const handleConversationComplete = () => {
    if (userInteractedRef.current) return;
    setActive((prev) => {
      const idx = toneSwitcher.modes.findIndex((m) => m.id === prev);
      const next = (idx + 1) % toneSwitcher.modes.length;
      return toneSwitcher.modes[next].id;
    });
  };

  const activeMode =
    toneSwitcher.modes.find((m) => m.id === active) ?? toneSwitcher.modes[0];

  return (
    <Section id="tones">
      <div className="grid lg:grid-cols-12 gap-8 lg:gap-10 items-center">
        {/* ─────────────── LEFT: headline + character-grid backdrop ────── */}
        <div className="relative lg:col-span-6">
          <div
            aria-hidden
            className="character-grid-bg absolute -inset-6 md:-inset-10 opacity-50 pointer-events-none"
            style={{
              maskImage:
                "radial-gradient(ellipse 70% 70% at 30% 50%, black 0%, transparent 80%)",
              WebkitMaskImage:
                "radial-gradient(ellipse 70% 70% at 30% 50%, black 0%, transparent 80%)",
            }}
          />
          <div className="relative">
            <p className="text-sm uppercase tracking-[0.18em] text-ink-200 mb-5 font-medium">
              {toneSwitcher.eyebrow}
            </p>
            <h2 className="font-display text-5xl md:text-6xl lg:text-7xl tracking-tightest leading-[0.98] text-balance text-ink-50">
              {toneSwitcher.title}
            </h2>
          </div>
        </div>

        {/* ─────────────── RIGHT: chat preview + tone tabs ────────────── */}
        <div className="lg:col-span-6">
          <Tabs.Root value={active} onValueChange={handleTabChange}>
            {/* Key on activeMode.id forces ChatPreview to remount on tone
                change — msgIndex + chars reset to 0 naturally, no stale state. */}
            <ChatPreview
              key={activeMode.id}
              messages={activeMode.messages}
              color={activeMode.color}
              colorRgb={activeMode.colorRgb}
              animate={animate}
              onConversationComplete={handleConversationComplete}
            />

            <Tabs.List
              className="mt-6 flex flex-wrap justify-center gap-2"
              aria-label="Tone"
            >
              {toneSwitcher.modes.map((m) => {
                const isActive = m.id === active;
                const styleVars = isActive
                  ? ({
                      background: `rgba(${m.colorRgb}, 0.16)`,
                      color: m.color,
                    } as CSSProperties)
                  : undefined;
                return (
                  <Tabs.Trigger
                    key={m.id}
                    value={m.id}
                    style={styleVars}
                    className={cn(
                      "rounded-full px-5 py-2 text-sm transition-colors duration-300",
                      "text-ink-200 hover:text-white hover:bg-ink-50/5",
                      "data-[state=active]:font-medium",
                      "focus-visible:outline-none",
                    )}
                  >
                    {m.label}
                  </Tabs.Trigger>
                );
              })}
            </Tabs.List>
          </Tabs.Root>
        </div>
      </div>
    </Section>
  );
}
