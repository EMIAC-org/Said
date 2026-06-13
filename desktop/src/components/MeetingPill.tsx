import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface EngineStatus {
  active: boolean;
  started_at_ms?: number | null;
}

function formatElapsed(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  return h > 0
    ? `${h}:${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`
    : `${m}:${String(s).padStart(2, "0")}`;
}

/**
 * The floating live-meeting pill. Always-on-top capsule shown while a meeting
 * records and the main window is minimized. Polls the engine for the recording
 * start time, ticks an elapsed timer, and restores the app on click.
 */
export function MeetingPill() {
  const [startedAt, setStartedAt] = useState<number | null>(null);
  const [, setTick] = useState(0);

  // Poll recording state every second.
  useEffect(() => {
    let cancelled = false;
    const poll = async () => {
      try {
        const status = await invoke<EngineStatus>("meeting_engine_get_status");
        if (!cancelled) setStartedAt(status.active ? (status.started_at_ms ?? null) : null);
      } catch {
        /* engine not ready */
      }
    };
    void poll();
    const id = window.setInterval(poll, 1000);
    return () => {
      cancelled = true;
      window.clearInterval(id);
    };
  }, []);

  // Re-render once a second to advance the timer.
  useEffect(() => {
    const id = window.setInterval(() => setTick((t) => t + 1), 1000);
    return () => window.clearInterval(id);
  }, []);

  const elapsed = startedAt ? Date.now() - startedAt : 0;

  return (
    <div
      onClick={() => void invoke("focus_main_from_pill")}
      title="Open AirNote"
      style={{
        display: "flex",
        alignItems: "center",
        gap: 10,
        height: "100%",
        width: "100%",
        padding: "0 16px",
        boxSizing: "border-box",
        cursor: "pointer",
        borderRadius: 26,
        background: "hsl(0 0% 8% / 0.92)",
        border: "1px solid hsl(0 0% 100% / 0.12)",
        boxShadow: "0 10px 30px hsl(0 0% 0% / 0.45)",
        backdropFilter: "blur(18px) saturate(150%)",
        WebkitBackdropFilter: "blur(18px) saturate(150%)",
        color: "white",
        fontFamily:
          "-apple-system, BlinkMacSystemFont, 'Segoe UI', system-ui, sans-serif",
        userSelect: "none",
      }}
    >
      <style>{`@keyframes airnote-pill-pulse {0%,100%{opacity:1}50%{opacity:.35}}`}</style>
      <span
        style={{
          width: 9,
          height: 9,
          borderRadius: "50%",
          background: "hsl(354 85% 62%)",
          flexShrink: 0,
          animation: "airnote-pill-pulse 1.4s ease-in-out infinite",
        }}
      />
      <span style={{ fontSize: 12, fontWeight: 700, letterSpacing: 0.2 }}>
        Recording
      </span>
      <span
        style={{
          fontSize: 13,
          fontWeight: 600,
          fontVariantNumeric: "tabular-nums",
          marginLeft: "auto",
          color: "hsl(0 0% 100% / 0.85)",
        }}
      >
        {formatElapsed(elapsed)}
      </span>
    </div>
  );
}
