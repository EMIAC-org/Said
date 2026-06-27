import { useEffect, useRef } from "react";

/**
 * BrandWave — the animated voice-waveform hero on the onboarding brand panel.
 *
 * A row of rounded bars under a smooth bell envelope that gently breathes, so
 * the left panel reads as "a living voice" rather than a static logo. Heights
 * are written straight to the DOM in a rAF loop (no React re-render per frame).
 * Respects prefers-reduced-motion by rendering a single static frame.
 */
const BAR_COUNT = 52;
const envelope = (t: number) => Math.pow(Math.sin(Math.PI * t), 0.62);

export function BrandWave() {
  const waveRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const wave = waveRef.current;
    if (!wave) return;
    const bars = Array.from(wave.children) as HTMLSpanElement[];
    const seed = bars.map((_, i) => i * 12.9898);

    const reduce =
      typeof window.matchMedia === "function" &&
      window.matchMedia("(prefers-reduced-motion: reduce)").matches;

    if (reduce) {
      bars.forEach((bar, i) => {
        const t = i / (bars.length - 1);
        bar.style.height = (6 + 112 * envelope(t) * 0.7).toFixed(1) + "px";
      });
      return;
    }

    let raf = 0;
    let amp = 0.2;
    const ampTarget = 0.2;
    const loop = (now: number) => {
      amp += (ampTarget - amp) * 0.08;
      const time = now / 1000;
      for (let i = 0; i < bars.length; i++) {
        const t = i / (bars.length - 1);
        const base = envelope(t);
        const osc = 0.5 + 0.5 * Math.sin(time * (2.2 + (i % 7) * 0.5) + seed[i]);
        const detail = 0.4 + 0.6 * osc;
        const h = 6 + 112 * base * detail * (0.32 + amp * 0.85);
        bars[i].style.height = h.toFixed(1) + "px";
      }
      raf = requestAnimationFrame(loop);
    };
    raf = requestAnimationFrame(loop);
    return () => cancelAnimationFrame(raf);
  }, []);

  return (
    <div className="onb-wave-wrap">
      <span className="onb-wave-halo" aria-hidden />
      <div className="onb-wave" ref={waveRef} aria-hidden>
        {Array.from({ length: BAR_COUNT }).map((_, i) => (
          <span key={i} className="onb-wave-bar" />
        ))}
      </div>
    </div>
  );
}
