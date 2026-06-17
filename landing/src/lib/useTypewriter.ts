"use client";

import { useEffect, useState } from "react";

type Options = {
  /** The string to reveal character by character. */
  text: string;
  /** Total loop length in ms (typewrite + hold + restart). */
  loopMs: number;
  /** How long the typewrite phase takes within the loop. */
  typeMs: number;
  /** When false, returns full length immediately (reduced-motion users). */
  enabled?: boolean;
};

/**
 * Reveals `text` one character at a time via requestAnimationFrame.
 * Holds at the complete string between `typeMs` and `loopMs`, then restarts.
 * Returns the current visible-character count.
 */
export function useTypewriter({
  text,
  loopMs,
  typeMs,
  enabled = true,
}: Options): number {
  const [chars, setChars] = useState(enabled ? 0 : text.length);

  useEffect(() => {
    if (!enabled) {
      setChars(text.length);
      return;
    }
    let raf = 0;
    const start = performance.now();
    const tick = (now: number) => {
      const elapsed = (now - start) % loopMs;
      const c = Math.min(
        text.length,
        Math.floor((elapsed / typeMs) * text.length),
      );
      setChars(c);
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [text, loopMs, typeMs, enabled]);

  return chars;
}
