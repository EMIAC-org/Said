import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

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
 * Floating live-meeting pill. Always-on-top capsule shown while a meeting
 * records and the app isn't in front. Restores the app on click, repositions on
 * drag. The timer runs from the meeting's actual start (latched from the engine,
 * with the pill's own mount time as a fallback) so it ticks immediately — it
 * never waits for the first transcript.
 */
export function MeetingPill() {
  // Latched meeting start. Once we learn the engine's real start time we keep it
  // (a transient active=false poll can't reset it). Falls back to mount time so
  // the timer always advances even before the first status poll lands.
  const fallbackStart = useRef<number>(Date.now());
  const [startedAt, setStartedAt] = useState<number | null>(null);
  const [, setTick] = useState(0);

  useEffect(() => {
    let cancelled = false;
    const poll = async () => {
      try {
        const status = await invoke<EngineStatus>("meeting_engine_get_status");
        if (cancelled) return;
        if (status.active && status.started_at_ms) {
          setStartedAt((prev) => prev ?? status.started_at_ms!); // latch once
        }
      } catch {
        /* engine not ready — keep ticking from the fallback */
      }
    };
    void poll();
    const id = window.setInterval(poll, 1000);
    return () => {
      cancelled = true;
      window.clearInterval(id);
    };
  }, []);

  useEffect(() => {
    const id = window.setInterval(() => setTick((t) => t + 1), 1000);
    return () => window.clearInterval(id);
  }, []);

  const elapsed = Date.now() - (startedAt ?? fallbackStart.current);

  // Drag vs click: track pointer movement on press; a real drag starts the
  // native window drag, a clean press (no movement) restores the app.
  const press = useRef<{ x: number; y: number; moved: boolean } | null>(null);
  const onMouseDown = (e: React.MouseEvent) => {
    if (e.button !== 0) return;
    press.current = { x: e.clientX, y: e.clientY, moved: false };
  };
  const onMouseMove = (e: React.MouseEvent) => {
    const p = press.current;
    if (!p || p.moved) return;
    if (Math.abs(e.clientX - p.x) > 4 || Math.abs(e.clientY - p.y) > 4) {
      p.moved = true;
      void getCurrentWindow().startDragging().catch(() => {});
    }
  };
  const onMouseUp = () => {
    const p = press.current;
    press.current = null;
    if (p && !p.moved) void invoke("focus_main_from_pill").catch(() => {});
  };

  return (
    <div
      onMouseDown={onMouseDown}
      onMouseMove={onMouseMove}
      onMouseUp={onMouseUp}
      title="Click to open AirNote · drag to move"
      style={{
        position: "fixed",
        inset: 0,
        display: "flex",
        alignItems: "center",
        gap: 10,
        padding: "0 16px",
        boxSizing: "border-box",
        cursor: "pointer",
        borderRadius: 26,
        overflow: "hidden",
        background: "hsl(0 0% 8% / 0.94)",
        border: "1px solid hsl(0 0% 100% / 0.12)",
        boxShadow: "0 10px 30px hsl(0 0% 0% / 0.45)",
        backdropFilter: "blur(18px) saturate(150%)",
        WebkitBackdropFilter: "blur(18px) saturate(150%)",
        color: "white",
        fontFamily: "-apple-system, BlinkMacSystemFont, 'Segoe UI', system-ui, sans-serif",
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
      <span style={{ fontSize: 12, fontWeight: 700, letterSpacing: 0.2 }}>Recording</span>
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
