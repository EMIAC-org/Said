import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
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
  // Always reflect the CURRENT meeting's start time from the engine, so the timer
  // is correct for every meeting — a freshly-started one reads its own start, not
  // a stale value latched from a previous meeting or the pill's mount time. Reset
  // when no meeting is active. (The pill window persists across meetings, so we
  // must re-sync rather than latch once.)
  const [startedAt, setStartedAt] = useState<number | null>(null);
  const [, setTick] = useState(0);

  useEffect(() => {
    let cancelled = false;
    const apply = (status: EngineStatus) => {
      if (cancelled) return;
      if (status.active && status.started_at_ms) {
        setStartedAt(status.started_at_ms);
      } else if (!status.active) {
        setStartedAt(null);
      }
    };
    // One-shot initial sync (the pill can mount mid-meeting), then EVENT-DRIVEN.
    // This pill is a SEPARATE webview window that shares the per-origin ipc://
    // connection pool with the main + status-bar windows. A 1s `get_status` poll
    // here (status() locks ~13 mutexes the live recorder holds) starved that pool
    // so "End meeting" (stop_session) could never get a connection to dispatch.
    // The backend already emits STATUS_EVENT on every change, so listen instead.
    invoke<EngineStatus>("meeting_engine_get_status").then(apply).catch(() => {});
    const unlisten = listen<EngineStatus>("meeting-engine-state", (e) => apply(e.payload));
    return () => {
      cancelled = true;
      unlisten.then((fn) => fn()).catch(() => {});
    };
  }, []);

  useEffect(() => {
    const id = window.setInterval(() => setTick((t) => t + 1), 1000);
    return () => window.clearInterval(id);
  }, []);

  const elapsed = startedAt ? Math.max(0, Date.now() - startedAt) : 0;

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

  // Equalizer bars — synthetic, gently animated so the pill always feels "alive"
  // (audio levels aren't streamed to this window). Staggered timings read organic.
  const EQ = [
    { d: "0.62s", delay: "0s" },
    { d: "0.94s", delay: "0.12s" },
    { d: "0.52s", delay: "0.30s" },
    { d: "0.80s", delay: "0.06s" },
  ];

  return (
    <div
      onMouseDown={onMouseDown}
      onMouseMove={onMouseMove}
      onMouseUp={onMouseUp}
      title="Click to open AirNote · drag to move"
      style={{
        // Capsule fills the window exactly. NO outer shadow/glow and NO
        // backdrop-filter — both bled into grey "corners" on the transparent
        // Windows window. Only a solid pill + a thin border + an inner highlight,
        // so the corners outside the rounded ends stay perfectly transparent.
        position: "fixed",
        inset: 0,
        display: "flex",
        alignItems: "center",
        gap: 11,
        padding: "0 16px",
        boxSizing: "border-box",
        cursor: "pointer",
        borderRadius: 26,
        overflow: "hidden",
        background:
          "linear-gradient(135deg, rgba(36,36,42,0.99) 0%, rgba(15,15,19,1) 100%)",
        border: "1px solid rgba(255,255,255,0.10)",
        boxShadow: "inset 0 1px 0 rgba(255,255,255,0.10)",
        color: "white",
        fontFamily:
          "-apple-system, BlinkMacSystemFont, 'Segoe UI', system-ui, sans-serif",
        userSelect: "none",
      }}
    >
      <style>{`
        @keyframes airnote-dot {0%,100%{transform:scale(1);opacity:1}50%{transform:scale(.82);opacity:.6}}
        @keyframes airnote-ring {0%{transform:scale(.7);opacity:.55}70%,100%{transform:scale(2.4);opacity:0}}
        @keyframes airnote-eq {0%,100%{transform:scaleY(.32)}50%{transform:scaleY(1)}}
      `}</style>

      {/* Glowing recording dot with an expanding halo ring */}
      <span style={{ position: "relative", width: 10, height: 10, flexShrink: 0 }}>
        <span
          style={{
            position: "absolute",
            inset: 0,
            borderRadius: "50%",
            background: "rgba(255,69,58,0.55)",
            animation: "airnote-ring 1.8s ease-out infinite",
          }}
        />
        <span
          style={{
            position: "absolute",
            inset: 0,
            borderRadius: "50%",
            background: "radial-gradient(circle at 35% 30%, #ff7a70, #ff3b30 70%)",
            boxShadow: "0 0 8px 1px rgba(255,69,58,0.85)",
            animation: "airnote-dot 1.8s ease-in-out infinite",
          }}
        />
      </span>

      <span
        style={{
          fontSize: 11.5,
          fontWeight: 600,
          letterSpacing: 0.3,
          color: "rgba(255,255,255,0.92)",
        }}
      >
        Recording
      </span>

      {/* Right-aligned: live equalizer + elapsed time */}
      <div
        style={{
          marginLeft: "auto",
          display: "flex",
          alignItems: "center",
          gap: 9,
        }}
      >
        <div style={{ display: "flex", alignItems: "center", gap: 2.5, height: 15 }}>
          {EQ.map((b, i) => (
            <span
              key={i}
              style={{
                width: 2.5,
                height: 15,
                borderRadius: 2,
                background: "linear-gradient(180deg, #ff7a70, #ff3b30)",
                transformOrigin: "center",
                animation: `airnote-eq ${b.d} ease-in-out ${b.delay} infinite`,
              }}
            />
          ))}
        </div>
        <span
          style={{
            fontSize: 13.5,
            fontWeight: 650,
            fontVariantNumeric: "tabular-nums",
            letterSpacing: 0.2,
            color: "#fff",
          }}
        >
          {formatElapsed(elapsed)}
        </span>
      </div>
    </div>
  );
}
