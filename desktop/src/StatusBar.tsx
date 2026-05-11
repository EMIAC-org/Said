import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow, LogicalPosition, LogicalSize, primaryMonitor } from "@tauri-apps/api/window";
import { Languages, RotateCcw, Sparkles, X } from "lucide-react";
import type { AppSnapshot } from "./types";

// ── State machine ─────────────────────────────────────────────────────────────

type BarState =
  | { kind: "idle" }
  | { kind: "recording"; startMs: number }
  | { kind: "processing"; phase: string }
  | { kind: "done" }
  | { kind: "pasted" }
  | { kind: "manual_paste" }
  | { kind: "error"; message: string; audioId?: string };

type VoiceErrorPayload = {
  message: string;
  audio_id?: string;
  error_code?: string;
  auto_hide_ms?: number;
};

type PillKind = BarState["kind"];
type HoverPanel = "language" | "tone" | null;

const BOTTOM_OFFSET = 64;
const HUD_CANVAS_WIDTH = 300;
const HUD_CANVAS_HEIGHT = 142;

const LEVEL_SHAPE = [0.32, 0.42, 0.58, 0.76, 0.92, 1.0, 0.86, 0.70, 0.86, 1.0, 0.92, 0.76, 0.58, 0.42, 0.32];
// Per-bar decay rates — edges decay faster than center so the visualizer looks naturally staggered
const BAR_DECAY = [0.72, 0.74, 0.76, 0.78, 0.80, 0.82, 0.80, 0.78, 0.80, 0.82, 0.80, 0.78, 0.76, 0.74, 0.72];
const LANG_OPTIONS = [
  { value: "hinglish", label: "Hinglish" },
  { value: "english", label: "English" },
  { value: "hindi", label: "Hindi" },
];
const TONE_OPTIONS = [
  { value: "professional", label: "Pro" },
  { value: "casual", label: "Casual" },
  { value: "concise", label: "Concise" },
  { value: "hinglish", label: "Hinglish" },
];

// ── Helpers ───────────────────────────────────────────────────────────────────

function pillSize(kind: PillKind, hasTranscript = false, hovered = false, hasPanel = false): { width: number; height: number } {
  if (hasPanel && hasTranscript) return { width: 300, height: 142 };
  if (hasPanel) return { width: 300, height: 80 };
  if (hasTranscript) return { width: 300, height: 102 };
  if (kind === "error") return { width: 300, height: 56 };
  if (kind === "idle" && hovered) return { width: 206, height: 40 };
  return { width: 184, height: 40 };
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
  const [idleHovered, setIdleHovered] = useState(false);
  const [liveTranscript, setLiveTranscript] = useState("");
  const [audioLevel, setAudioLevel] = useState(0);
  const [hoverPanel, setHoverPanel] = useState<HoverPanel>(null);
  const [outputLanguage, setOutputLanguage] = useState("hinglish");
  const [tonePreset, setTonePreset] = useState("professional");
  const doneTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const audioLevelRef = useRef(0);
  const barTargets = useRef<number[]>(new Array(15).fill(0));
  const [, forceFrame] = useState(0);
  const win = getCurrentWindow();
  const hasTranscript = bar.kind === "processing" && liveTranscript.trim().length > 0;
  const hasPanel = hoverPanel !== null && bar.kind !== "error";
  const innerSize = pillSize(bar.kind, hasTranscript, bar.kind === "idle" && idleHovered, hasPanel);

  useEffect(() => {
    console.info("[status-bar] mounted", {
      label: win.label,
      href: window.location.href,
      hash: window.location.hash,
      search: window.location.search,
    });
  }, []);

  // VoiceInk uses a max-size native panel and expands the inner capsule inside it.
  // Keep our native Tauri window at the largest HUD size so hover panels are never clipped.
  useEffect(() => {
    console.info("[status-bar] state", bar);
    primaryMonitor()
      .then((monitor) => {
        const scale = monitor?.scaleFactor ?? 1;
        const sw = monitor ? monitor.size.width / scale : 1440;
        const sh = monitor ? monitor.size.height / scale : 900;
        const sx = monitor ? monitor.position.x / scale : 0;
        const sy = monitor ? monitor.position.y / scale : 0;
        const x = sx + sw / 2 - HUD_CANVAS_WIDTH / 2;
        const y = sy + sh - HUD_CANVAS_HEIGHT - BOTTOM_OFFSET;
        return win
          .setSize(new LogicalSize(HUD_CANVAS_WIDTH, HUD_CANVAS_HEIGHT))
          .then(() => win.setPosition(new LogicalPosition(x, y)));
      })
      .then(() => console.info("[status-bar] chrome sized", { width: HUD_CANVAS_WIDTH, height: HUD_CANVAS_HEIGHT }))
      .catch((err) => console.warn("[status-bar] chrome size failed", err));
  }, []);

  useEffect(() => {
    if (bar.kind !== "recording") return;
    let raf = 0;
    const tick = () => {
      const lvl = audioLevelRef.current;
      barTargets.current = barTargets.current.map((cur, i) => {
        // Each bar gets a different random slice of the audio energy so bars
        // look independent even though they share one RMS signal.
        const target = lvl * LEVEL_SHAPE[i] * (0.80 + Math.random() * 0.40);
        // Attack immediately, decay at each bar's own rate.
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
      setHoverPanel(null);
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
    invoke<{ output_language: string; tone_preset: string }>("get_preferences")
      .then((prefs) => {
        if (prefs.output_language) setOutputLanguage(prefs.output_language);
        if (prefs.tone_preset) setTonePreset(prefs.tone_preset);
      })
      .catch((err) => console.warn("[status-bar] prefs fetch failed", err));
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

    // ── Success: flash "Done" for 1.8 s then hide ──────────────────────────
    listen("voice-done", () => {
      console.info("[status-bar] voice-done event");
      if (doneTimer.current) clearTimeout(doneTimer.current);
      setBar({ kind: "done" });
      doneTimer.current = setTimeout(() => setBar({ kind: "idle" }), 2400);
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
        e.payload.status === "pasted" ? 1800 : 5200,
      );
    }).then((fn) => {
      console.info("[status-bar] subscribed voice-output");
      subs.push(fn);
    }).catch((err) => console.warn("[status-bar] voice-output subscribe failed", err));

    // ── Error: show message + optional retry ──────────────────────────────
    listen<VoiceErrorPayload>("voice-error", (e) => {
      const { message, audio_id, auto_hide_ms } = e.payload;
      console.info("[status-bar] voice-error event", { message, hasAudioId: Boolean(audio_id), autoHideMs: auto_hide_ms });
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

    listen("prefs-changed", () => {
      invoke<{ output_language: string; tone_preset: string }>("get_preferences")
        .then((prefs) => {
          if (prefs.output_language) setOutputLanguage(prefs.output_language);
          if (prefs.tone_preset) setTonePreset(prefs.tone_preset);
        })
        .catch(() => {});
    }).then((fn) => {
      subs.push(fn);
    }).catch(() => {});

    return () => {
      console.info("[status-bar] unmount subscriptions", subs.length);
      subs.forEach((fn) => fn());
    };
  }, []);

  useEffect(() => () => { if (doneTimer.current) clearTimeout(doneTimer.current); }, []);

  async function patchPref(update: Record<string, string>) {
    try {
      const prefs = await invoke<{ output_language: string; tone_preset: string }>("patch_preferences", { update });
      if (prefs.output_language) setOutputLanguage(prefs.output_language);
      if (prefs.tone_preset) setTonePreset(prefs.tone_preset);
    } catch (err) {
      console.warn("[status-bar] patch_preferences failed", err);
    }
  }

  return (
    <div
      className={`sb-shell sb-shell--${bar.kind}${hasTranscript ? " sb-shell--expanded" : ""}${hasPanel ? " sb-shell--with-panel" : ""}${bar.kind === "idle" && idleHovered ? " sb-shell--hovered" : ""}`}
      style={{ width: innerSize.width, height: innerSize.height }}
      aria-label={`Said ${bar.kind}`}
      title={`Said ${bar.kind}`}
      onMouseEnter={() => {
        if (bar.kind === "idle") setIdleHovered(true);
      }}
      onMouseLeave={() => {
        setIdleHovered(false);
        setHoverPanel(null);
      }}
    >
      {hasTranscript && (
        <div className="sb-transcript">
          {liveTranscript}
        </div>
      )}

      {hasPanel && (
        <div className="sb-hover-panel">
          {(hoverPanel === "language" ? LANG_OPTIONS : TONE_OPTIONS).map((option) => {
            const active = hoverPanel === "language"
              ? option.value === outputLanguage
              : option.value === tonePreset;
            return (
              <button
                key={option.value}
                className={`sb-chip${active ? " sb-chip--active" : ""}`}
                onClick={() => {
                  if (hoverPanel === "language") {
                    setOutputLanguage(option.value);
                    void patchPref({ output_language: option.value });
                  } else {
                    setTonePreset(option.value);
                    void patchPref({ tone_preset: option.value });
                  }
                }}
              >
                {option.label}
              </button>
            );
          })}
        </div>
      )}

      <div className="sb-controlbar">
        <button
          className={`sb-icon-btn${hoverPanel === "language" ? " sb-icon-btn--active" : ""}`}
          title="Output language"
          aria-label="Output language"
          onMouseEnter={() => setHoverPanel("language")}
          onFocus={() => setHoverPanel("language")}
        >
          <Languages size={13} />
        </button>

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

        {bar.kind === "error" ? (
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
        ) : (
          <button
            className={`sb-icon-btn${hoverPanel === "tone" ? " sb-icon-btn--active" : ""}`}
            title="Tone mode"
            aria-label="Tone mode"
            onMouseEnter={() => setHoverPanel("tone")}
            onFocus={() => setHoverPanel("tone")}
          >
            <Sparkles size={13} />
          </button>
        )}
      </div>

    </div>
  );
}
