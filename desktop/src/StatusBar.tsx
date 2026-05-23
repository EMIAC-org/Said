import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { RotateCcw, X } from "lucide-react";
import type { AppSnapshot } from "./types";
import type { CSSProperties } from "react";

// ── State machine ─────────────────────────────────────────────────────────────

type BarState =
  | { kind: "idle" }
  | { kind: "recording"; startMs: number }
  | { kind: "processing"; phase: string }
  | { kind: "done" }
  | { kind: "pasted" }
  | { kind: "manual_paste" }
  | { kind: "error"; message: string; audioId?: string }
  | { kind: "learned"; term: string; message: string };

type VoiceErrorPayload = {
  message: string;
  audio_id?: string;
  error_code?: string;
  auto_hide_ms?: number;
};

type PillKind = BarState["kind"];

type StatusBarMetrics = {
  has_notch: boolean;
  window_width: number;
  window_height: number;
  closed_width: number;
  closed_height: number;
  hover_width: number;
  hover_height: number;
  recording_width: number;
  recording_height: number;
  processing_width: number;
  processing_height: number;
  transcript_width: number;
  transcript_height: number;
  done_width: number;
  done_height: number;
  learned_width: number;
  learned_height: number;
  error_width: number;
  error_height: number;
  top_radius: number;
  bottom_radius: number;
  expanded_bottom_radius: number;
};

const DEFAULT_METRICS: StatusBarMetrics = {
  has_notch: false,
  window_width: 540,
  window_height: 144,
  closed_width: 184,
  closed_height: 32,
  hover_width: 218,
  hover_height: 40,
  recording_width: 232,
  recording_height: 60,
  processing_width: 238,
  processing_height: 58,
  transcript_width: 500,
  transcript_height: 114,
  done_width: 212,
  done_height: 52,
  learned_width: 302,
  learned_height: 52,
  error_width: 326,
  error_height: 52,
  top_radius: 999,
  bottom_radius: 999,
  expanded_bottom_radius: 24,
};

const LEVEL_SHAPE = [0.28, 0.38, 0.52, 0.68, 0.82, 1.0, 0.78, 0.62, 0.78, 1.0, 0.82, 0.68, 0.52, 0.38, 0.28];
const BAR_DECAY = [0.82, 0.84, 0.85, 0.86, 0.87, 0.88, 0.87, 0.86, 0.87, 0.88, 0.87, 0.86, 0.85, 0.84, 0.82];

// ── Helpers ───────────────────────────────────────────────────────────────────

function pillSize(
  kind: PillKind,
  metrics: StatusBarMetrics,
  hasTranscript = false,
  hovered = false,
): { width: number; height: number } {
  if (hasTranscript) return { width: metrics.transcript_width, height: metrics.transcript_height };
  if (kind === "learned") return { width: metrics.learned_width, height: metrics.learned_height };
  if (kind === "error") return { width: metrics.error_width, height: metrics.error_height };
  if (kind === "recording") return { width: metrics.recording_width, height: metrics.recording_height };
  if (kind === "processing") return { width: metrics.processing_width, height: metrics.processing_height };
  if (kind === "done" || kind === "pasted" || kind === "manual_paste") return { width: metrics.done_width, height: metrics.done_height };
  if (kind === "idle" && hovered) return { width: metrics.hover_width, height: metrics.hover_height };
  return { width: metrics.closed_width, height: metrics.closed_height };
}

function processingLabel(phase: string): string {
  const p = phase.toLowerCase();
  if (p.includes("polish") || p.includes("llm") || p.includes("enhanc")) return "Enhancing";
  if (p.includes("paste")) return "Pasting";
  return "Transcribing";
}

function barHeight(barLevel: number, active: boolean): number {
  if (!active) return 4;
  return 4 + barLevel * 24;
}

// ── Component ─────────────────────────────────────────────────────────────────

export default function StatusBar() {
  const [bar, setBar] = useState<BarState>({ kind: "idle" });
  const [metrics, setMetrics] = useState<StatusBarMetrics>(DEFAULT_METRICS);
  const [idleHovered, setIdleHovered] = useState(false);
  const [liveTranscript, setLiveTranscript] = useState("");
  const [audioLevel, setAudioLevel] = useState(0);
  const doneTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const audioLevelRef = useRef(0);
  const barTargets = useRef<number[]>(new Array(15).fill(0));
  const [, forceFrame] = useState(0);
  const win = getCurrentWindow();
  const hasTranscript = bar.kind === "processing" && liveTranscript.trim().length > 0;
  const innerSize = pillSize(bar.kind, metrics, hasTranscript, bar.kind === "idle" && idleHovered);
  const shellStyle = {
    width: innerSize.width,
    height: innerSize.height,
    "--sb-top-radius": `${metrics.top_radius}px`,
    "--sb-bottom-radius": `${metrics.bottom_radius}px`,
    "--sb-expanded-bottom-radius": `${metrics.expanded_bottom_radius}px`,
  } as CSSProperties;

  useEffect(() => {
    console.info("[status-bar] mounted", {
      label: win.label,
      href: window.location.href,
      hash: window.location.hash,
      search: window.location.search,
    });
  }, []);

  // Native Rust/AppKit owns window placement. React only receives the measured
  // notch/fallback sizes and animates the HUD surface inside that canvas.
  useEffect(() => {
    let unsubscribe: (() => void) | undefined;
    invoke<StatusBarMetrics>("get_status_bar_metrics")
      .then((next) => {
        console.info("[status-bar] metrics initial", next);
        setMetrics(next);
      })
      .catch((err) => console.warn("[status-bar] metrics fetch failed", err));

    listen<StatusBarMetrics>("status-bar-metrics", (event) => {
      setMetrics(event.payload);
    })
      .then((fn) => {
        unsubscribe = fn;
      })
      .catch((err) => console.warn("[status-bar] metrics subscribe failed", err));

    return () => {
      unsubscribe?.();
    };
  }, []);

  useEffect(() => {
    if (bar.kind !== "recording") return;
    let raf = 0;
    const tick = () => {
      // sqrt compression tames peaks — voice at 0.6 raw → 0.77 compressed,
      // but full-scale 1.0 stays 1.0. Keeps bars proportional without clipping.
      const raw = audioLevelRef.current;
      const lvl = Math.sqrt(raw) * 0.7;
      barTargets.current = barTargets.current.map((cur, i) => {
        const target = lvl * LEVEL_SHAPE[i] * (0.90 + Math.random() * 0.20);
        return Math.max(cur * BAR_DECAY[i], target);
      });
      forceFrame((n) => (n + 1) % 1000);
      raf = window.requestAnimationFrame(tick);
    };
    raf = window.requestAnimationFrame(tick);
    return () => {
      window.cancelAnimationFrame(raf);
      barTargets.current = new Array(15).fill(0);
    };
  }, [bar.kind]);

  useEffect(() => {
    if (bar.kind === "idle") {
      setIdleHovered(false);
    }
  }, [bar.kind]);

  // Seed from current snapshot on mount so we reflect any in-progress state
  useEffect(() => {
    invoke<AppSnapshot>("get_snapshot")
      .then((snap) => {
        console.info("[status-bar] initial snapshot", snap.state);
        if (snap.state === "recording") {
          setBar({ kind: "recording", startMs: Date.now() });
        } else if (snap.state === "processing") {
          setBar({ kind: "processing", phase: "stt" });
        }
      })
      .catch((err) => {
        console.warn("[status-bar] initial snapshot failed", err);
      });
  }, []);

  useEffect(() => {
    const subs: Array<() => void> = [];

    // ── Source of truth for recording / processing / idle ──────────────────
    listen<AppSnapshot>("app-state", (e) => {
      const { state } = e.payload;
      console.info("[status-bar] app-state event", state);
      if (state === "recording") {
        if (doneTimer.current) clearTimeout(doneTimer.current);
        setLiveTranscript("");
        setAudioLevel(0);
        setBar({ kind: "recording", startMs: Date.now() });
      } else if (state === "processing") {
        setBar((prev) =>
          prev.kind === "recording"
            ? { kind: "processing", phase: "stt" }
            : prev.kind === "processing" ? prev
            : { kind: "processing", phase: "stt" }
        );
        if (doneTimer.current) clearTimeout(doneTimer.current);
        doneTimer.current = setTimeout(() => {
          setBar((prev) => prev.kind === "processing" ? { kind: "idle" } : prev);
        }, 15000);
      } else if (state === "idle") {
        // Only auto-hide if we're not waiting on a user-action (error/done)
        setBar((prev) => {
          if (prev.kind === "error") return prev; // user must dismiss
          if (prev.kind === "done" || prev.kind === "pasted" || prev.kind === "manual_paste") {
            return prev; // timer handles it
          }
          return { kind: "idle" };
        });
      }
    }).then((fn) => {
      console.info("[status-bar] subscribed app-state");
      subs.push(fn);
    }).catch((err) => console.warn("[status-bar] app-state subscribe failed", err));

    // ── Sub-phase label updates ────────────────────────────────────────────
    listen<{ phase: string; transcript?: string }>("voice-status", (e) => {
      const { phase, transcript } = e.payload;
      console.info("[status-bar] voice-status event", phase);
      if (transcript?.trim()) setLiveTranscript(transcript.trim());
      setBar((prev) =>
        prev.kind === "processing" ? { kind: "processing", phase } : prev
      );
    }).then((fn) => {
      console.info("[status-bar] subscribed voice-status");
      subs.push(fn);
    }).catch((err) => console.warn("[status-bar] voice-status subscribe failed", err));

    listen<{ level: number }>("voice-level", (e) => {
      const level = Number.isFinite(e.payload.level) ? e.payload.level : 0;
      const clamped = Math.max(0, Math.min(1, level));
      audioLevelRef.current = clamped;
      setAudioLevel(clamped);
    }).then((fn) => {
      console.info("[status-bar] subscribed voice-level");
      subs.push(fn);
    }).catch((err) => console.warn("[status-bar] voice-level subscribe failed", err));

    // ── Success: brief flash then hide ──────────────────────────────────────
    listen("voice-done", () => {
      console.info("[status-bar] voice-done event");
      if (doneTimer.current) clearTimeout(doneTimer.current);
      setBar({ kind: "done" });
      doneTimer.current = setTimeout(() => setBar({ kind: "idle" }), 100);
    }).then((fn) => {
      console.info("[status-bar] subscribed voice-done");
      subs.push(fn);
    }).catch((err) => console.warn("[status-bar] voice-done subscribe failed", err));

    listen<{ status: "pasted" | "manual_paste"; message?: string }>("voice-output", (e) => {
      console.info("[status-bar] voice-output event", e.payload);
      if (doneTimer.current) clearTimeout(doneTimer.current);
      setBar({ kind: e.payload.status });
      doneTimer.current = setTimeout(
        () => setBar({ kind: "idle" }),
        e.payload.status === "pasted" ? 100 : 5200,
      );
    }).then((fn) => {
      console.info("[status-bar] subscribed voice-output");
      subs.push(fn);
    }).catch((err) => console.warn("[status-bar] voice-output subscribe failed", err));

    // ── Error: show message + optional retry ──────────────────────────────
    listen<VoiceErrorPayload & { raw_error?: string }>("voice-error", (e) => {
      const { message, audio_id, auto_hide_ms, raw_error } = e.payload;
      console.error("[status-bar] voice-error event", { message, raw_error, hasAudioId: Boolean(audio_id) });
      if (doneTimer.current) clearTimeout(doneTimer.current);
      win.show().catch((err) => console.warn("[status-bar] show failed for error", err));
      setBar({ kind: "error", message, audioId: audio_id });
      if (typeof auto_hide_ms === "number" && auto_hide_ms > 0) {
        doneTimer.current = setTimeout(() => setBar({ kind: "idle" }), auto_hide_ms);
      }
    }).then((fn) => {
      console.info("[status-bar] subscribed voice-error");
      subs.push(fn);
    }).catch((err) => console.warn("[status-bar] voice-error subscribe failed", err));

    // ── Learning: show term in status bar with undo ──────────────────
    listen<{ term: string; message: string }>("vocab-learned", (e) => {
      console.info("[status-bar] vocab-learned", e.payload);
      if (doneTimer.current) clearTimeout(doneTimer.current);
      win.show().catch(() => {});
      setBar({ kind: "learned", term: e.payload.term, message: e.payload.message });
      doneTimer.current = setTimeout(() => {
        setBar({ kind: "idle" });
        invoke("dismiss_status_bar").catch(() => {});
      }, 3000);
    }).then((fn) => {
      subs.push(fn);
    }).catch(() => {});

    return () => {
      console.info("[status-bar] unmount subscriptions", subs.length);
      subs.forEach((fn) => fn());
    };
  }, []);

  useEffect(() => () => { if (doneTimer.current) clearTimeout(doneTimer.current); }, []);



  return (
    <div
      className={`sb-shell sb-shell--${bar.kind}${hasTranscript ? " sb-shell--expanded" : ""}${bar.kind === "idle" && idleHovered ? " sb-shell--hovered" : ""}${metrics.has_notch ? "" : " sb-shell--floating"}`}
      style={shellStyle}
      aria-label={`Said ${bar.kind}`}
      title={`Said ${bar.kind}`}
      onMouseEnter={() => {
        if (bar.kind === "idle") setIdleHovered(true);
      }}
      onMouseLeave={() => {
        setIdleHovered(false);
      }}
    >
      {hasTranscript && (
        <div className="sb-transcript">
          {liveTranscript}
        </div>
      )}

      <div className="sb-controlbar">
        <div className="sb-center">
          {bar.kind === "processing" ? (
            <div className="sb-processing">
              <span>{processingLabel(bar.phase)}</span>
              <span className="sb-progress-dots" aria-hidden="true">
                <span />
                <span />
                <span />
                <span />
                <span />
              </span>
            </div>
          ) : bar.kind === "done" || bar.kind === "pasted" ? (
            <div className="sb-success" aria-hidden="true">
              <span />
              <span />
              <span />
            </div>
          ) : bar.kind === "manual_paste" ? (
            <div className="sb-manual">
              <span />
            </div>
          ) : bar.kind === "learned" ? (
            <div className="sb-learned">
              <span className="sb-learned-dot" />
              <span className="sb-learned-text"><strong>{bar.term}</strong> learned</span>
            </div>
          ) : bar.kind === "error" ? (
            <div className="sb-error-copy">
              <span className="sb-error-pulse" />
              <span>{bar.message}</span>
            </div>
          ) : (
            <div className={`sb-visualizer${bar.kind === "recording" ? " sb-visualizer--active" : ""}`}>
              {Array.from({ length: 15 }).map((_, index) => (
                <span
                  key={index}
                  style={{
                    height: `${barHeight(barTargets.current[index], bar.kind === "recording")}px`,
                    opacity: bar.kind === "recording" ? 0.54 + audioLevel * 0.46 : 0.5,
                  }}
                />
              ))}
            </div>
          )}
        </div>

        {bar.kind === "learned" ? (
          <button
            className="sb-undo-btn"
            title="Undo — remove this term"
            aria-label="Undo"
            onClick={async () => {
              try {
                await invoke("delete_vocabulary_term", { term: bar.term });
              } catch (e) {
                console.warn("[status-bar] delete_vocab_term failed", e);
              }
              setBar({ kind: "idle" });
              invoke("dismiss_status_bar").catch(() => {});
            }}
          >
            <RotateCcw size={10} />
            <span>Undo</span>
          </button>
        ) : bar.kind === "error" ? (
          <div className="sb-error-actions">
            {bar.audioId && (
              <button
                className="sb-icon-btn sb-icon-btn--retry"
                title="Retry"
                aria-label="Retry"
                onClick={async () => {
                  try {
                    await invoke("retry_recording", { audioId: bar.audioId });
                    setBar({ kind: "processing", phase: "stt" });
                  } catch (e) {
                    setBar({ kind: "error", message: String(e) });
                  }
                }}
              >
                <RotateCcw size={12} />
              </button>
            )}
            <button
              className="sb-icon-btn sb-icon-btn--dismiss"
              title="Dismiss"
              aria-label="Dismiss"
              onClick={() => {
                setBar({ kind: "idle" });
                invoke("dismiss_status_bar").catch((err) => {
                  console.warn("[status-bar] dismiss failed", err);
                });
              }}
            >
              <X size={13} />
            </button>
          </div>
        ) : null}
      </div>

    </div>
  );
}
