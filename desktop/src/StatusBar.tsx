import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { emit, listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow, LogicalPosition, LogicalSize } from "@tauri-apps/api/window";
import { Copy, Download, RotateCcw, X } from "lucide-react";
import type { AppSnapshot } from "./types";
import {
  APPLY_UPDATE_FAILED_EVENT,
  getPendingReadyUpdateReminder,
  requestApplyPendingUpdate,
  snoozeReadyUpdateReminder,
} from "./lib/autoUpdate";
import {
  developerProblemChooseProject,
  developerProblemDismiss,
  onDeveloperContext,
  type DeveloperContextCandidate,
} from "./lib/invoke";

function notifEnabled(key: string): boolean {
  try {
    const raw = localStorage.getItem("airnote-notif-prefs");
    if (!raw) return true;
    const prefs = JSON.parse(raw);
    return prefs[key] !== false;
  } catch { return true; }
}

function soundsEnabled(): boolean { return notifEnabled("sounds"); }

// ── Sound synthesis (Web Audio, no external files) ───────────────────────────

let _audioCtx: AudioContext | null = null;
function getAudioCtx(): AudioContext {
  if (!_audioCtx) _audioCtx = new AudioContext();
  if (_audioCtx.state === "suspended") _audioCtx.resume();
  return _audioCtx;
}

function osc(freq: number, type: OscillatorType, vol: number, dur: number, delay = 0) {
  const ctx = getAudioCtx();
  const o = ctx.createOscillator();
  const g = ctx.createGain();
  o.type = type;
  o.frequency.value = freq;
  g.gain.setValueAtTime(0, ctx.currentTime + delay);
  g.gain.linearRampToValueAtTime(vol, ctx.currentTime + delay + 0.01);
  g.gain.exponentialRampToValueAtTime(0.001, ctx.currentTime + delay + dur);
  o.connect(g);
  g.connect(ctx.destination);
  o.start(ctx.currentTime + delay);
  o.stop(ctx.currentTime + delay + dur);
}

function oscSweep(from: number, to: number, type: OscillatorType, vol: number, dur: number) {
  const ctx = getAudioCtx();
  const o = ctx.createOscillator();
  const g = ctx.createGain();
  o.type = type;
  o.frequency.setValueAtTime(from, ctx.currentTime);
  o.frequency.exponentialRampToValueAtTime(to, ctx.currentTime + dur * 0.5);
  g.gain.setValueAtTime(vol, ctx.currentTime);
  g.gain.exponentialRampToValueAtTime(0.001, ctx.currentTime + dur);
  o.connect(g);
  g.connect(ctx.destination);
  o.start();
  o.stop(ctx.currentTime + dur);
}

const sounds = {
  chimeUp:   () => { osc(660, "sine", 0.1, 0.12, 0); osc(880, "sine", 0.1, 0.12, 0.07); },
  chimeDown: () => { osc(880, "sine", 0.1, 0.1, 0); osc(660, "sine", 0.1, 0.1, 0.06); },
  ding:      () => { osc(1046, "sine", 0.08, 0.15, 0); osc(1318, "sine", 0.08, 0.15, 0.06); },
  whoosh:    () => { oscSweep(440, 1760, "sine", 0.06, 0.15); },
  lowThud:   () => { oscSweep(220, 110, "triangle", 0.12, 0.2); },
  levelUp:   () => { osc(523, "sine", 0.09, 0.2, 0); osc(659, "sine", 0.09, 0.2, 0.08); osc(784, "sine", 0.09, 0.2, 0.16); },
  knock:     () => { osc(330, "triangle", 0.1, 0.06, 0); osc(330, "triangle", 0.1, 0.06, 0.1); },
  alert:     () => { osc(440, "triangle", 0.1, 0.12, 0); osc(330, "triangle", 0.1, 0.12, 0.1); },
  shimmer:   () => { osc(784, "sine", 0.05, 0.3, 0); osc(988, "sine", 0.05, 0.3, 0.04); osc(1175, "sine", 0.05, 0.3, 0.08); osc(1568, "sine", 0.05, 0.3, 0.12); },
  tick:      () => { osc(1200, "sine", 0.06, 0.06); },
} as const;

const RECOVERY_PREVIEW_ENABLED =
  import.meta.env.VITE_AIRNOTE_RECOVERY_PREVIEW === "1" ||
  new URLSearchParams(window.location.search).get("recoveryPreview") === "1";

type SoundName = keyof typeof sounds;

function playSound(name: SoundName | null) {
  if (!name || !soundsEnabled()) return;
  try { sounds[name](); } catch { /* audio context not ready */ }
}

// ── State machine ─────────────────────────────────────────────────────────────

type BarState =
  | { kind: "idle" }
  | { kind: "recording"; startMs: number }
  | { kind: "processing"; phase: string }
  | { kind: "done" }
  | { kind: "pasted" }
  | { kind: "manual_paste"; message?: string }
  | { kind: "error"; message: string; runId?: string; audioId?: string; rawError?: string; errorCode?: string; diagnostic?: string }
  | { kind: "recovered"; text: string; copied: boolean }
  | { kind: "learned"; message: string; wordId?: number }
  | { kind: "placement"; message: string }
  | { kind: "polish_mode"; enabled: boolean; message: string }
  | { kind: "problem_ambiguous"; candidates: DeveloperContextCandidate[] }
  | { kind: "update_ready"; version: string; message: string };

type UpdateReadyState = Extract<BarState, { kind: "update_ready" }>;

type VoiceErrorPayload = {
  message: string;
  run_id?: string;
  audio_id?: string;
  error_code?: string;
  raw_error?: string;
  diagnostic?: string;
  auto_hide_ms?: number;
};

type VoiceStatusPayload = {
  phase: string;
  transcript?: string | null;
  run_id?: string | null;
  recording_id?: string | null;
};

type PillKind = BarState["kind"];

function keepsHudOverIdle(kind: PillKind): boolean {
  return kind === "error"
    || kind === "done"
    || kind === "pasted"
    || kind === "manual_paste"
    || kind === "polish_mode"
    || kind === "problem_ambiguous"
    || kind === "update_ready"
    || kind === "recovered";
}

const HUD_CANVAS_MIN_WIDTH = 300;
// The compact voice pill hugs the waveform (15 bars ≈ 85px incl. padding), so it
// stays a tight capsule. It widens only when the "Polish" badge rides alongside
// the wave during recording.
const VOICE_COMPACT_WIDTH = 108;
const VOICE_COMPACT_POLISH_WIDTH = 150;
const VOICE_INNER_WIDTH = 280;
const VOICE_COMPACT_HEIGHT = 34;
const VOICE_INNER_HEIGHT = 98;
const VOICE_CANVAS_WIDTH = VOICE_INNER_WIDTH + 40;
const VOICE_CANVAS_HEIGHT = VOICE_INNER_HEIGHT + 40;

// Fluid waveform: a parabolic envelope (tall center, short edges). Bars animate
// via scaleY around this envelope — a traveling wave that tracks the mic level
// while recording, an indeterminate sweep while processing. See the waveform
// driver effect + the visualizer JSX. WAVE_REST is the resting (silent) scale.
const WAVE_ENVELOPE = Array.from({ length: 15 }, (_, i) => {
  const d = Math.abs(i - 7) / 7;
  return 0.35 + (1 - d * d) * 0.65;
});
const WAVE_REST = 0.14;

// ── Helpers ───────────────────────────────────────────────────────────────────

function textWidth(text: string): number {
  return Math.ceil(text.length * 6.8);
}

function pillSize(
  kind: PillKind,
  hasTranscript = false,
  label = "",
  actionCount = 0,
): { width: number; height: number } {
  if (hasTranscript) return { width: VOICE_INNER_WIDTH, height: VOICE_INNER_HEIGHT };
  if (kind === "problem_ambiguous") return { width: 360, height: 176 };
  if (kind === "error") {
    const actionWidth = actionCount > 0
      ? (actionCount * 22) + ((actionCount - 1) * 6) + 8
      : 0;
    const content = Math.min(textWidth(label), 300) + actionWidth + 42;
    return { width: Math.max(240, Math.min(Math.ceil(content), 460)), height: 40 };
  }
  if (kind === "recovered") return { width: 380, height: 196 };
  if (kind === "update_ready") {
    const content = Math.min(textWidth(label), 190) + 148;
    return { width: Math.max(320, Math.min(Math.ceil(content), 390)), height: 136 };
  }

  if (label) {
    const content = textWidth(label) + 8 + 18 + 18;
    const padded = Math.ceil(content * 1.2);
    return { width: Math.max(120, Math.min(padded, 340)), height: 36 };
  }

  return { width: 140, height: 36 };
}

function processingLabel(phase: string): string {
  const p = phase.toLowerCase();
  if (p.startsWith("using context:")) return phase;
  if (p === "no project context") return "No Project Context";
  if (p.includes("problem_transcribing")) return "Transcribing problem";
  if (p.includes("problem_solving")) return "Solving problem";
  if (p.includes("server_audio_fallback")) return "Using local runtime";
  if (p.includes("server_transcrib") || p.includes("server-audio")) return "Server transcribing";
  if (p.includes("server_polish") || p.includes("server-polish") || p.includes("server_polishing")) return "Server polish";
  if (p.includes("message_polish") || p.includes("message-polish")) return "Polishing message";
  if (p.includes("polish") || p.includes("llm") || p.includes("enhanc")) return "Enhancing";
  if (p.includes("paste")) return "Pasting";
  return "Transcribing";
}

function CardHost({
  children,
  variant = "anchored",
}: {
  children: ReactNode;
  variant?: "anchored" | "pill" | "card";
}) {
  return (
    <div className={`sb-expand-host sb-expand-host--${variant}`}>
      {children}
    </div>
  );
}

// ── Component ─────────────────────────────────────────────────────────────────

export default function StatusBar() {
  const [bar, setBar] = useState<BarState>(() => ({ kind: "idle" }));
  const [dragUnlocked, setDragUnlocked] = useState(false);
  const dragActiveRef = useRef(false);
  const [liveTranscript, setLiveTranscript] = useState("");
  const [audioLevel, setAudioLevel] = useState(0);
  const [polishModeEnabled, setPolishModeEnabled] = useState(false);
  const [longDictationLocked, setLongDictationLocked] = useState(false);
  const doneTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const audioLevelRef = useRef(0);
  const pinnedUpdateRef = useRef<UpdateReadyState | null>(null);
  const barTargets = useRef<number[]>(new Array(15).fill(0));
  const lastResizeRef = useRef<{ width: number; height: number } | null>(null);
  const barKindRef = useRef<BarState["kind"]>("idle");
  const currentRunIdRef = useRef<string | null>(null);
  const [, forceFrame] = useState(0);
  const [win] = useState(() => getCurrentWindow());
  const normalizeRunId = (value?: string | null): string | null => {
    const trimmed = value?.trim();
    return trimmed && trimmed.length > 0 ? trimmed : null;
  };
  const setCurrentRunId = (runId: string | null) => {
    if (currentRunIdRef.current !== runId) {
      currentRunIdRef.current = runId;
      setLiveTranscript("");
    }
  };
  const clearRunTranscript = () => {
    currentRunIdRef.current = null;
    setLiveTranscript("");
  };
  const isStaleTerminalRun = (runId: string | null): boolean => {
    const current = currentRunIdRef.current;
    if (runId) {
      if (current && current !== runId) return true;
      return !current && barKindRef.current === "idle";
    }
    return current !== null;
  };
  const presentStatusBar = (reason: string) => {
    invoke("present_status_bar", { reason }).catch((err) => {
      console.warn("[status-bar] native present failed", err);
      win.show().catch((showErr) => console.warn("[status-bar] fallback show failed", showErr));
    });
  };
  const showPinnedUpdate = (next: UpdateReadyState, reason: string) => {
    pinnedUpdateRef.current = next;
    if (doneTimer.current) clearTimeout(doneTimer.current);
    invoke("set_status_bar_persistent", { persistent: true, reason }).catch((err) => {
      console.warn("[status-bar] persistent hold failed", err);
      presentStatusBar(reason);
    });
    setBar(next);
  };
  const restorePinnedUpdate = (reason: string): boolean => {
    const pinned = pinnedUpdateRef.current;
    if (!pinned) return false;
    if (doneTimer.current) clearTimeout(doneTimer.current);
    presentStatusBar(reason);
    setBar(pinned);
    return true;
  };
  const returnToIdleOrPinned = (reason: string, dismiss = true) => {
    if (restorePinnedUpdate(reason)) return;
    setBar({ kind: "idle" });
    if (dismiss) {
      invoke("dismiss_status_bar").catch(() => {});
    }
  };
  const clearPinnedUpdate = async (reason: string) => {
    pinnedUpdateRef.current = null;
    try {
      await invoke("set_status_bar_persistent", { persistent: false, reason });
    } catch (err) {
      console.warn("[status-bar] clear persistent hold failed", err);
    }
  };
  const hasTranscript =
    (bar.kind === "recording" || bar.kind === "processing") && liveTranscript.trim().length > 0;
  const isInteractive =
    bar.kind === "error"
    || bar.kind === "learned"
    || bar.kind === "update_ready"
    || bar.kind === "recovered"
    || bar.kind === "placement"
    || bar.kind === "problem_ambiguous";

  const pillLabel = (() => {
    switch (bar.kind) {
      case "idle": return "AirNote";
      case "recording": return "Recording";
      case "processing": return bar.phase;
      case "done": return "Done";
      case "pasted": return "Pasted";
      case "manual_paste": return bar.message || "Paste latest";
      case "error": return bar.message;
      case "recovered": return "Recovered dictation";
      case "learned": return bar.message;
      case "placement": return bar.message;
      case "polish_mode": return bar.message;
      case "problem_ambiguous": return "Ambiguous Project Match";
      case "update_ready": return `Update ${bar.version} ready`;
      default: return "";
    }
  })();

  const compactActionCount = bar.kind === "error" ? 2 + (bar.audioId ? 2 : 0) : 0;
  const innerSize = pillSize(
    bar.kind,
    hasTranscript,
    pillLabel,
    compactActionCount,
  );

  useEffect(() => {
    barKindRef.current = bar.kind;
  }, [bar.kind]);

  useEffect(() => {
    const root = document.documentElement;
    const app = document.getElementById("app");
    if (bar.kind !== "idle") {
      root?.classList.add("sb-card-mode");
      app?.classList.remove("sb-app--card");
    } else {
      root?.classList.remove("sb-card-mode");
      app?.classList.remove("sb-app--card");
    }
    return () => {
      root?.classList.remove("sb-card-mode");
      app?.classList.remove("sb-app--card");
    };
  }, [bar.kind]);

  useEffect(() => {
    invoke("set_status_bar_interactive", {
      interactive: isInteractive || dragUnlocked,
    }).catch(() => {});
  }, [isInteractive, dragUnlocked]);

  // Hold Left Option (⌥) / Left Alt + drag to reposition — saved to disk, restored on every show.
  useEffect(() => {
    const app = document.getElementById("app");
    if (!app) return;

    const savePosition = () => {
      Promise.all([win.outerPosition(), win.scaleFactor()])
        .then(([pos, scale]) => {
          invoke("set_status_bar_position", {
            x: pos.x / scale,
            y: pos.y / scale,
          }).catch(() => {});
        })
        .catch(() => {});
    };

    const arm = () => {
      setDragUnlocked(true);
      document.documentElement.classList.add("sb-drag-unlocked");
    };

    const disarm = () => {
      if (dragActiveRef.current) savePosition();
      dragActiveRef.current = false;
      setDragUnlocked(false);
      document.documentElement.classList.remove("sb-drag-unlocked", "sb-dragging");
    };

    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape" && document.documentElement.classList.contains("sb-drag-unlocked")) {
        e.preventDefault();
        disarm();
        setBar({ kind: "idle" });
        invoke("dismiss_status_bar").catch(() => {});
        return;
      }
      if (e.code === "AltLeft" && !e.repeat) arm();
    };
    const onKeyUp = (e: KeyboardEvent) => {
      if (e.code === "AltLeft" || e.key === "Alt") disarm();
    };

    const onMouseDown = (e: MouseEvent) => {
      if (e.button !== 0) return;
      if (!e.altKey && !document.documentElement.classList.contains("sb-drag-unlocked")) return;
      e.preventDefault();
      arm();
      dragActiveRef.current = true;
      document.documentElement.classList.add("sb-dragging");
      win.startDragging().catch(() => {});
    };

    const onMouseUp = () => {
      if (!dragActiveRef.current) return;
      dragActiveRef.current = false;
      document.documentElement.classList.remove("sb-dragging");
      savePosition();
    };

    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("keyup", onKeyUp);
    window.addEventListener("blur", disarm);
    app.addEventListener("mousedown", onMouseDown, true);
    window.addEventListener("mouseup", onMouseUp);

    return () => {
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("keyup", onKeyUp);
      window.removeEventListener("blur", disarm);
      app.removeEventListener("mousedown", onMouseDown, true);
      window.removeEventListener("mouseup", onMouseUp);
      disarm();
    };
  }, [win]);

  useEffect(() => {
    if (!dragUnlocked) return;
    const app = document.getElementById("app");
    if (!app || app.querySelector(".sb-drag-overlay")) return;

    const overlay = document.createElement("div");
    overlay.className = "sb-drag-overlay";
    overlay.setAttribute("aria-hidden", "true");
    const hint = document.createElement("span");
    hint.className = "sb-drag-hint";
    const isWin = typeof navigator !== "undefined" && /Win/i.test(navigator.userAgent);
    hint.textContent = isWin
      ? "Drag to move · Shift+Ctrl+/ to finish"
      : "Drag to move · ⇧⌘/ to finish";
    overlay.appendChild(hint);
    app.appendChild(overlay);

    return () => {
      overlay.remove();
    };
  }, [dragUnlocked]);

  useEffect(() => {
    invoke<{ x: number; y: number } | null>("get_status_bar_position")
      .then((pos) => {
        if (!pos) return;
        win.setPosition(new LogicalPosition(pos.x, pos.y)).catch(() => {});
      })
      .catch(() => {});
  }, [win]);

  // Resize native window before paint — voice states use a fixed canvas so the
  // transcript grows upward inside the panel instead of pushing the window down.
  useLayoutEffect(() => {
    const usesVoiceCanvas = bar.kind === "recording" || bar.kind === "processing";
    const w = usesVoiceCanvas
      ? VOICE_CANVAS_WIDTH
      : Math.max(innerSize.width + 40, HUD_CANVAS_MIN_WIDTH);
    const h = usesVoiceCanvas
      ? VOICE_CANVAS_HEIGHT
      : Math.max(innerSize.height + 40, 56);

    const apply = (force = false) => {
      const previous = lastResizeRef.current;
      if (!force && previous?.width === w && previous?.height === h) return;
      lastResizeRef.current = { width: w, height: h };
      invoke("resize_status_bar", { width: w, height: h }).catch(() => {
        win.setSize(new LogicalSize(w, h)).catch(() => {});
      });
    };

    apply();
  }, [innerSize.width, innerSize.height, bar.kind, win]);

  useEffect(() => {
    console.info("[status-bar] mounted", {
      label: win.label,
      href: window.location.href,
      hash: window.location.hash,
      search: window.location.search,
    });
  }, []);

  // Web Lock: prevents WKWebView from suspending JS on macOS 13 (Sonoma+
  // already handles this via BackgroundThrottlingPolicy::Disabled, but 13 ignores it).
  // visibilitychange: re-sync app state if the WebView was throttled while hidden.
  useEffect(() => {
    if (typeof navigator?.locks?.request === "function") {
      navigator.locks.request(
        "airnote-statusbar-keepalive",
        { mode: "shared" },
        () => new Promise<void>(() => {}),
      );
    }
    const onVisibilityChange = () => {
      if (document.visibilityState === "visible") {
        invoke<AppSnapshot>("get_snapshot")
          .then((snap) => applyActiveSnapshot(snap, "visibility-restored"))
          .catch(() => {});
      }
    };
    document.addEventListener("visibilitychange", onVisibilityChange);
    return () => {
      document.removeEventListener("visibilitychange", onVisibilityChange);
    };
  }, []);

  // VoiceInk uses a max-size native panel and expands the inner capsule inside it.
  // Keep our native Tauri window at the largest HUD size so hover panels are never clipped.
  // No fixed mount sizing — the content-driven resize effect handles everything.

  // Fluid waveform driver. Recording → a traveling wave whose peaks track the
  // live mic level; processing → an indeterminate sweep sweeping left→right,
  // brighter/faster for the AI-polish phase and dimmer/slower for transcription.
  // Writes per-bar scaleY into barTargets and forces a frame; the visualizer
  // reads barTargets.current[i]. sqrt compression tames peaks without clipping.
  useEffect(() => {
    const isRecording = bar.kind === "recording";
    const isProcessing = bar.kind === "processing";
    if (!isRecording && !isProcessing) return;
    const bright =
      isProcessing && /polish|llm|enhanc/.test((bar.kind === "processing" ? bar.phase : "").toLowerCase());
    const count = WAVE_ENVELOPE.length;
    let raf = 0;
    let start = 0;
    const tick = (now: number) => {
      if (!start) start = now;
      const t = (now - start) / 1000;
      const raw = audioLevelRef.current;
      const lvl = Math.min(1, Math.sqrt(raw) * 0.95);
      barTargets.current = WAVE_ENVELOPE.map((env, i) => {
        if (isRecording) {
          const travel = 0.5 + 0.5 * Math.sin(t * 6 - i * 0.55);
          const peak = WAVE_REST + lvl * env * (1 - WAVE_REST);
          return WAVE_REST + (peak - WAVE_REST) * travel;
        }
        const speed = bright ? 1.9 : 1.2;
        const span = bright ? 1.2 : 1.0;
        const width = bright ? 3.5 : 6;
        const amp = bright ? 0.85 : 0.5;
        const pos = (t * speed) % span;
        const d = Math.abs(i / (count - 1) - pos);
        return WAVE_REST + Math.max(0, 1 - d * width) * env * amp;
      });
      forceFrame((n) => (n + 1) % 1000);
      raf = window.requestAnimationFrame(tick);
    };
    raf = window.requestAnimationFrame(tick);
    return () => {
      window.cancelAnimationFrame(raf);
      barTargets.current = new Array(count).fill(WAVE_REST);
    };
  }, [bar.kind, bar.kind === "processing" ? bar.phase : null]);


  // Auto-hide the native window when returning to idle.
  useEffect(() => {
    if (bar.kind !== "idle") return;
    const t = setTimeout(() => {
      invoke("dismiss_status_bar").catch(() => {});
    }, 500);
    return () => clearTimeout(t);
  }, [bar.kind]);

  // Seed from current snapshot on mount so we reflect any in-progress state
  const applyActiveSnapshot = (snap: AppSnapshot, source: string) => {
    console.info("[status-bar] snapshot resync", source, snap.state);
    setPolishModeEnabled(Boolean(snap.message_polish_mode));
    const runId = normalizeRunId(snap.recording_id);
    if (snap.state === "recording") {
      setCurrentRunId(runId);
      setBar((prev) =>
        prev.kind === "recording"
          ? prev
          : { kind: "recording", startMs: Date.now() },
      );
    } else if (snap.state === "processing") {
      setLongDictationLocked(false);
      if (runId) setCurrentRunId(runId);
      setBar((prev) =>
        prev.kind === "processing"
          ? prev
          : { kind: "processing", phase: "stt" },
      );
    } else if (snap.state === "idle") {
      setLongDictationLocked(false);
      clearRunTranscript();
      if (restorePinnedUpdate(`auto-update-ready-${source}-idle`)) return;
      setBar((prev) => {
        if (keepsHudOverIdle(prev.kind)) return prev;
        return { kind: "idle" };
      });
    }
  };

  useEffect(() => {
    invoke<AppSnapshot>("get_snapshot")
      .then((snap) => applyActiveSnapshot(snap, "mount"))
      .catch((err) => {
        console.warn("[status-bar] initial snapshot failed", err);
      });
  }, []);

  useEffect(() => {
    let alive = true;
    void getPendingReadyUpdateReminder().then((version) => {
      if (!alive || !version) return;
      showPinnedUpdate({
        kind: "update_ready",
        version,
        message: `Update ${version} is ready. Restart AirNote to use it.`,
      }, "auto-update-ready-restored");
    }).catch((err) => {
      console.warn("[status-bar] failed to restore pending update", err);
    });
    return () => {
      alive = false;
    };
  }, []);

  useEffect(() => {
    if (!RECOVERY_PREVIEW_ENABLED) return;
    const text = [
      "This is a recovered dictation preview from AirNote.",
      "The card should stay in the status bar, follow light and dark theme colors, and keep the main app clean.",
    ].join(" ");
    presentStatusBar("dictation-recovered-preview");
    setBar({ kind: "recovered", text, copied: false });
  }, []);

  useEffect(() => {
    const subs: Array<() => void> = [];

    listen<{ reason?: string; state?: string }>("status-bar-resync", (e) => {
      console.info("[status-bar] resync event", e.payload);
      invoke<AppSnapshot>("get_snapshot")
        .then((snap) => applyActiveSnapshot(snap, e.payload?.reason || "event"))
        .catch((err) => console.warn("[status-bar] resync snapshot failed", err));
    }).then((fn) => {
      subs.push(fn);
    }).catch((err) => console.warn("[status-bar] resync subscribe failed", err));

    // ── Source of truth for recording / processing / idle ──────────────────
    listen<AppSnapshot>("app-state", (e) => {
      const { state } = e.payload;
      console.info("[status-bar] app-state event", state);
      setPolishModeEnabled(Boolean(e.payload.message_polish_mode));
      const runId = normalizeRunId(e.payload.recording_id);
      if (state === "recording") {
        setCurrentRunId(runId);
        if (barKindRef.current !== "recording") {
          if (doneTimer.current) clearTimeout(doneTimer.current);
          setAudioLevel(0);
          playSound("chimeUp");
          barKindRef.current = "recording";
        }
        setBar((prev) => (
          prev.kind === "recording"
            ? prev
            : { kind: "recording", startMs: Date.now() }
        ));
      } else if (state === "processing") {
        setLongDictationLocked(false);
        if (runId) setCurrentRunId(runId);
        setBar((prev) =>
          prev.kind === "recording"
            ? { kind: "processing", phase: "stt" }
            : prev.kind === "processing" ? prev
            : { kind: "processing", phase: "stt" }
        );
        if (doneTimer.current) clearTimeout(doneTimer.current);
        doneTimer.current = setTimeout(() => {
          if (restorePinnedUpdate("auto-update-ready-after-processing-timeout")) return;
          clearRunTranscript();
          setBar((prev) => prev.kind === "processing" ? { kind: "idle" } : prev);
        }, 15000);
      } else if (state === "idle") {
        setLongDictationLocked(false);
        clearRunTranscript();
        if (restorePinnedUpdate("auto-update-ready-idle")) return;
        setBar((prev) => {
          if (keepsHudOverIdle(prev.kind)) return prev;
          return { kind: "idle" };
        });
      }
    }).then((fn) => {
      console.info("[status-bar] subscribed app-state");
      subs.push(fn);
    }).catch((err) => console.warn("[status-bar] app-state subscribe failed", err));

    // ── Sub-phase label updates ────────────────────────────────────────────
    listen<VoiceStatusPayload>("voice-status", (e) => {
      const { phase, transcript } = e.payload;
      console.info("[status-bar] voice-status event", phase);
      const runId = normalizeRunId(e.payload.run_id ?? e.payload.recording_id);
      const currentRunId = currentRunIdRef.current;
      if (runId) {
        if (currentRunId && currentRunId !== runId) return;
        if (!currentRunId) {
          if (barKindRef.current !== "recording" && barKindRef.current !== "processing") return;
          currentRunIdRef.current = runId;
        }
      } else if (currentRunId || barKindRef.current === "idle") {
        return;
      }
      if (transcript?.trim()) setLiveTranscript(transcript.trim());
      setBar((prev) => {
        if (prev.kind === "recording" && phase === "live_stt") {
          return prev;
        }
        return prev.kind === "processing"
          ? (prev.phase === phase ? prev : { kind: "processing", phase })
          : prev;
      });
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
    listen<{ run_id?: string | null }>("voice-done", (e) => {
      const runId = normalizeRunId(e.payload?.run_id);
      if (isStaleTerminalRun(runId)) return;
      console.info("[status-bar] voice-done event");
      clearRunTranscript();
      if (restorePinnedUpdate("auto-update-ready-after-done")) return;
      if (doneTimer.current) clearTimeout(doneTimer.current);
      setBar({ kind: "done" });
      doneTimer.current = setTimeout(() => {
        setBar((prev) => prev.kind === "done" ? { kind: "idle" } : prev);
      }, 1500);
    }).then((fn) => {
      console.info("[status-bar] subscribed voice-done");
      subs.push(fn);
    }).catch((err) => console.warn("[status-bar] voice-done subscribe failed", err));

    listen<{ status: "pasted" | "manual_paste"; message?: string; run_id?: string | null }>("voice-output", (e) => {
      const runId = normalizeRunId(e.payload.run_id);
      if (isStaleTerminalRun(runId)) return;
      console.info("[status-bar] voice-output event", e.payload);
      clearRunTranscript();
      if (restorePinnedUpdate("auto-update-ready-after-output")) return;
      if (doneTimer.current) clearTimeout(doneTimer.current);
      playSound("whoosh");
      setBar(
        e.payload.status === "manual_paste"
          ? { kind: "manual_paste", message: e.payload.message }
          : { kind: "pasted" },
      );
      doneTimer.current = setTimeout(
        () => setBar({ kind: "idle" }),
        e.payload.status === "pasted" ? 100 : 5200,
      );
    }).then((fn) => {
      console.info("[status-bar] subscribed voice-output");
      subs.push(fn);
    }).catch((err) => console.warn("[status-bar] voice-output subscribe failed", err));

    subs.push(onDeveloperContext((payload) => {
      console.info("[status-bar] problem-command-context", payload);
      if (doneTimer.current) clearTimeout(doneTimer.current);
      if (payload.outcome === "ambiguous") {
        presentStatusBar("problem-ambiguous");
        playSound("alert");
        invoke("set_status_bar_persistent", {
          persistent: true,
          reason: "problem-ambiguous",
          interactive: true,
        }).catch(() => presentStatusBar("problem-ambiguous"));
        setBar({ kind: "problem_ambiguous", candidates: payload.candidates });
        win.setFocus().catch(() => {});
        return;
      }
      playSound(payload.outcome === "project" ? "tick" : "chimeDown");
      setBar({ kind: "processing", phase: payload.label });
    }));

    listen<{ enabled: boolean; message?: string }>("message-polish-mode", (e) => {
      console.info("[status-bar] message-polish-mode event", e.payload);
      if (restorePinnedUpdate("auto-update-ready-after-polish-mode")) return;
      if (doneTimer.current) clearTimeout(doneTimer.current);
      presentStatusBar("message-polish-mode");
      playSound(e.payload.enabled ? "levelUp" : "tick");
      setPolishModeEnabled(e.payload.enabled);
      setBar({
        kind: "polish_mode",
        enabled: e.payload.enabled,
        message: e.payload.message || (e.payload.enabled ? "Polish mode on" : "Polish mode off"),
      });
      doneTimer.current = setTimeout(() => returnToIdleOrPinned("message-polish-mode-hide"), 3200);
    }).then((fn) => {
      console.info("[status-bar] subscribed message-polish-mode");
      subs.push(fn);
    }).catch((err) => console.warn("[status-bar] message-polish-mode subscribe failed", err));

    // ── Error: show message + optional retry ──────────────────────────────
    listen<VoiceErrorPayload>("voice-error", (e) => {
      const { message, run_id, audio_id, auto_hide_ms, raw_error, error_code, diagnostic } = e.payload;
      const runId = normalizeRunId(run_id);
      if (runId && isStaleTerminalRun(runId)) return;
      console.error("[status-bar] voice-error event", {
        message,
        raw_error,
        error_code,
        diagnostic,
        hasAudioId: Boolean(audio_id),
      });
      clearRunTranscript();
      if (doneTimer.current) clearTimeout(doneTimer.current);
      if (!notifEnabled("error")) return;
      presentStatusBar("voice-error");
      playSound("lowThud");
      setBar({ kind: "error", message, runId: run_id, audioId: audio_id, rawError: raw_error, errorCode: error_code, diagnostic });
      if (typeof auto_hide_ms === "number" && auto_hide_ms > 0) {
        doneTimer.current = setTimeout(
          () => returnToIdleOrPinned("auto-update-ready-after-error", false),
          auto_hide_ms,
        );
      }
    }).then((fn) => {
      console.info("[status-bar] subscribed voice-error");
      subs.push(fn);
    }).catch((err) => console.warn("[status-bar] voice-error subscribe failed", err));

    listen<{ text: string }>("dictation-recovered", (e) => {
      const text = e.payload?.text?.trim() || "";
      if (!text) return;
      console.info("[status-bar] dictation-recovered event", { chars: text.length });
      if (doneTimer.current) clearTimeout(doneTimer.current);
      presentStatusBar("dictation-recovered");
      playSound("ding");
      setBar({ kind: "recovered", text, copied: false });
    }).then((fn) => {
      console.info("[status-bar] subscribed dictation-recovered");
      subs.push(fn);
    }).catch((err) => console.warn("[status-bar] dictation-recovered subscribe failed", err));

    listen("long-dictation-locked", () => {
      console.info("[status-bar] long-dictation-locked event");
      setLongDictationLocked(true);
      if (barKindRef.current === "recording") {
        presentStatusBar("long-dictation-locked");
      }
    }).then((fn) => {
      subs.push(fn);
    }).catch((err) => console.warn("[status-bar] long-dictation subscribe failed", err));

    listen("long-dictation-unlocked", () => {
      console.info("[status-bar] long-dictation-unlocked event");
      setLongDictationLocked(false);
    }).then((fn) => {
      subs.push(fn);
    }).catch((err) => console.warn("[status-bar] long-dictation unlock subscribe failed", err));

    listen<{ message?: string }>("status-bar-placement-mode", (e) => {
      console.info("[status-bar] placement mode event", e.payload);
      if (doneTimer.current) clearTimeout(doneTimer.current);
      setBar({ kind: "placement", message: e.payload?.message || "Drag AirNote" });
      document.documentElement.classList.add("sb-drag-unlocked");
      setDragUnlocked(true);
    }).then((fn) => {
      subs.push(fn);
    }).catch((err) => console.warn("[status-bar] placement mode subscribe failed", err));

    listen("status-bar-placement-finish", () => {
      console.info("[status-bar] placement finish event");
      dragActiveRef.current = false;
      document.documentElement.classList.remove("sb-drag-unlocked", "sb-dragging");
      setDragUnlocked(false);
      Promise.all([win.outerPosition(), win.scaleFactor()])
        .then(([pos, scale]) =>
          invoke("set_status_bar_position", { x: pos.x / scale, y: pos.y / scale }),
        )
        .catch(() => {});
      if (!restorePinnedUpdate("auto-update-ready-after-placement")) {
        setBar({ kind: "idle" });
      }
    }).then((fn) => {
      subs.push(fn);
    }).catch((err) => console.warn("[status-bar] placement finish subscribe failed", err));

    // ── Learning notifications ────────────────────────────────────────
    listen<{ message: string; id?: number }>("vocab-learned", (e) => {
      if (!notifEnabled("learned")) return;
      console.info("[status-bar] vocab-learned", e.payload);
      if (doneTimer.current) clearTimeout(doneTimer.current);
      presentStatusBar("vocab-learned");
      playSound("levelUp");
      setBar({ kind: "learned", message: e.payload.message, wordId: e.payload.id });
      doneTimer.current = setTimeout(() => {
        setBar({ kind: "idle" });
        invoke("dismiss_status_bar").catch(() => {});
      }, 3000);
    }).then((fn) => {
      subs.push(fn);
    }).catch(() => {});

    listen<{ version: string; message?: string }>("auto-update-ready", (e) => {
      if (!notifEnabled("updates")) return;
      console.info("[status-bar] auto-update-ready", e.payload);
      playSound("shimmer");
      showPinnedUpdate({
        kind: "update_ready",
        version: e.payload.version,
        message: e.payload.message || `Update ${e.payload.version} downloaded. Restart AirNote to use it.`,
      }, "auto-update-ready");
    }).then((fn) => {
      subs.push(fn);
    }).catch(() => {});

    listen<{ message?: string }>(APPLY_UPDATE_FAILED_EVENT, (e) => {
      if (!notifEnabled("updates")) return;
      const version = pinnedUpdateRef.current?.version || "the update";
      showPinnedUpdate({
        kind: "update_ready",
        version,
        message: `Restart failed. Try again from Settings. ${e.payload?.message || ""}`.trim(),
      }, "auto-update-restart-failed");
    }).then((fn) => {
      subs.push(fn);
    }).catch(() => {});

    return () => {
      console.info("[status-bar] unmount subscriptions", subs.length);
      subs.forEach((fn) => fn());
    };
  }, []);

  useEffect(() => () => { if (doneTimer.current) clearTimeout(doneTimer.current); }, []);

  // Signal the backend that all event listeners are registered.
  // Runs after the listener-registration useEffect above (effects run in order).
  // A small delay covers the async IPC round-trip inside each listen() call.
  // The backend re-emits status-bar-resync so any state set during startup is not missed.
  useEffect(() => {
    const t = window.setTimeout(() => {
      emit("frontend-ready").catch(() => {});
    }, 100);
    return () => window.clearTimeout(t);
  }, []);

  async function releaseProblemHold(reason: string) {
    try {
      await invoke("set_status_bar_persistent", { persistent: false, reason });
    } catch {
      // The solve path can still continue; this only controls HUD click capture.
    }
  }

  async function chooseProblemProject(project: DeveloperContextCandidate) {
    try {
      await developerProblemChooseProject(project.id);
      await releaseProblemHold("problem-choice");
      setBar({ kind: "processing", phase: `Using Context: ${project.name}` });
    } catch (e) {
      await releaseProblemHold("problem-choice-error");
      setBar({ kind: "error", message: e instanceof Error ? e.message : String(e) });
    }
  }

  async function dismissProblemAmbiguity(openSettings: boolean) {
    try {
      await developerProblemDismiss();
    } catch {
      // Dismiss is best-effort; clearing local UI state is still safe.
    }
    await releaseProblemHold(openSettings ? "problem-edit-aliases" : "problem-dismiss");
    setBar({ kind: "idle" });
    if (openSettings) {
      invoke("focus_main_from_pill").catch(() => {});
      emit("nav-settings", { section: "developer" }).catch(() => {});
    } else {
      invoke("dismiss_status_bar").catch(() => {});
    }
  }

  // ── Idle: nothing visible; native window hides via dismiss_status_bar ──
  if (bar.kind === "idle") {
    return null;
  }

  // ── Developer Problem Command: hard stop on ambiguous project context ──
  if (bar.kind === "problem_ambiguous") {
    return (
      <CardHost>
        <div
          className="sb-survey sb-survey--panel sb-survey--interactive"
          style={{ width: innerSize.width, height: innerSize.height }}
          aria-label="Ambiguous project match"
        >
          <div className="sb-survey-kicker-row">
            <span className="sb-status-dot sb-status-dot--warn" />
            <span className="sb-survey-kicker">Ambiguous Project Match</span>
            <button
              className="sb-card-close"
              title="Dismiss"
              aria-label="Dismiss"
              onClick={() => { void dismissProblemAmbiguity(false); }}
            >
              <X size={13} />
            </button>
          </div>
          <div className="sb-survey-body">
            Pick the project context to continue. No answer is generated until you choose.
          </div>
          <div className="sb-choice-chips" style={{ maxHeight: 56 }}>
            {bar.candidates.map((candidate) => (
              <button
                key={candidate.id}
                type="button"
                className="sb-choice-chip"
                title={candidate.matched_alias ? `Matched "${candidate.matched_alias}"` : candidate.name}
                onClick={() => { void chooseProblemProject(candidate); }}
              >
                {candidate.name}
              </button>
            ))}
          </div>
          <div className="sb-card-foot">
            <span className="sb-card-hint">Context hard stop</span>
            <button
              type="button"
              className="sb-survey-skip"
              onClick={() => { void dismissProblemAmbiguity(true); }}
            >
              Edit Aliases
            </button>
          </div>
        </div>
      </CardHost>
    );
  }

  if (bar.kind === "update_ready") {
    return (
      <CardHost>
        <div
          className="sb-survey sb-survey--panel sb-survey--interactive"
          style={{ width: innerSize.width, height: innerSize.height }}
          aria-label="AirNote update ready"
        >
          <div className="sb-survey-kicker-row">
            <span className="sb-status-dot sb-status-dot--ok" />
            <span className="sb-survey-kicker">Update downloaded</span>
          </div>
          <div className="sb-survey-body">
            {bar.message}
          </div>
          <div className="sb-survey-footer">
            <button
              type="button"
              className="sb-survey-skip"
              onClick={async () => {
                snoozeReadyUpdateReminder();
                await clearPinnedUpdate("auto-update-later");
                setBar({ kind: "idle" });
                invoke("dismiss_status_bar").catch(() => {});
              }}
            >
              Later
            </button>
            <button
              type="button"
              className="sb-survey-next"
              onClick={async () => {
                try {
                  showPinnedUpdate({
                    ...bar,
                    message: `Applying update ${bar.version}…`,
                  }, "auto-update-restart-requested");
                  await requestApplyPendingUpdate();
                } catch (err) {
                  const message = err instanceof Error ? err.message : String(err);
                  showPinnedUpdate({
                    ...bar,
                    message: `Restart failed. Try again, or open Settings > About. ${message}`,
                  }, "auto-update-restart-failed");
                }
              }}
            >
              Restart
              <RotateCcw size={14} strokeWidth={2} aria-hidden="true" />
            </button>
          </div>
        </div>
      </CardHost>
    );
  }

  if (bar.kind === "recovered") {
    return (
      <CardHost>
        <div
          className="sb-survey sb-survey--panel sb-survey--interactive"
          style={{ width: innerSize.width, height: innerSize.height }}
          aria-label="AirNote recovered dictation"
        >
          <div className="sb-survey-kicker-row">
            <span className="sb-status-dot sb-status-dot--info" />
            <span className="sb-survey-kicker">Recovered your last dictation</span>
          </div>
          <div className="sb-survey-body sb-recovered-body">
            AirNote recovered audio from the previous run.
          </div>
          <div className="sb-recovered-text">
            {bar.text}
          </div>
          <div className="sb-survey-footer">
            <button
              type="button"
              className="sb-survey-skip"
              onClick={() => {
                setBar({ kind: "idle" });
                invoke("dismiss_status_bar").catch(() => {});
              }}
            >
              Dismiss
            </button>
            <button
              type="button"
              className="sb-survey-next"
              onClick={() => {
                navigator.clipboard.writeText(bar.text)
                  .then(() => {
                    setBar((prev) => prev.kind === "recovered" ? { ...prev, copied: true } : prev);
                  })
                  .catch((err) => console.warn("[status-bar] copy recovered dictation failed", err));
              }}
            >
              {bar.copied ? "Copied" : "Copy"}
              <Copy size={14} strokeWidth={2} aria-hidden="true" />
            </button>
          </div>
        </div>
      </CardHost>
    );
  }

  const usesVoiceCanvas = bar.kind === "recording" || bar.kind === "processing";
  const voiceSurfaceWidth = hasTranscript
    ? VOICE_INNER_WIDTH
    : bar.kind === "recording" && polishModeEnabled
      ? VOICE_COMPACT_POLISH_WIDTH
      : VOICE_COMPACT_WIDTH;
  const voiceSurfaceHeight = hasTranscript ? VOICE_INNER_HEIGHT : VOICE_COMPACT_HEIGHT;

  return (
    <CardHost>
      <div
        className={`sb-survey sb-survey--compact${usesVoiceCanvas ? " sb-survey--voice" : ""}${hasTranscript ? " sb-survey--tall" : ""}${isInteractive ? " sb-survey--interactive" : ""}`}
        style={{ width: usesVoiceCanvas ? voiceSurfaceWidth : innerSize.width, height: usesVoiceCanvas ? voiceSurfaceHeight : innerSize.height }}
        aria-label={`AirNote ${bar.kind}`}
      >
        {usesVoiceCanvas && (
          <div className={`sb-survey-transcript${hasTranscript ? " sb-survey-transcript--open" : ""}`}>
            {liveTranscript}
          </div>
        )}

        <div className={`sb-survey-controlbar${bar.kind === "error" ? " sb-survey-controlbar--actions" : ""}`}>
          {bar.kind === "done" || bar.kind === "pasted" ? (
            <div className="sb-survey-success" aria-hidden="true">
              <span />
              <span />
              <span />
            </div>
          ) : bar.kind === "manual_paste" ? (
            <div className="sb-manual">
              <span />
            </div>
          ) : bar.kind === "learned" ? (
            <div className="sb-survey-label">
              <span className="sb-status-dot sb-status-dot--ok" />
              <span>{bar.message}</span>
            </div>
          ) : bar.kind === "error" ? (
            <div className="sb-survey-label">
              <span className="sb-status-dot sb-status-dot--err" />
              <span>{bar.message}</span>
            </div>
          ) : bar.kind === "placement" ? (
            <div className="sb-survey-label">
              <span className="sb-status-dot sb-status-dot--info" />
              <span>{bar.message}</span>
            </div>
          ) : bar.kind === "polish_mode" ? (
            <div className="sb-survey-label">
              <span className={`sb-status-dot ${bar.enabled ? "sb-status-dot--ok" : "sb-status-dot--info"}`} />
              <span>{bar.message}</span>
            </div>
          ) : (
            <div
              className={`sb-survey-visualizer sb-wave sb-wave--${
                bar.kind === "recording"
                  ? "listening"
                  : bar.kind === "processing"
                    ? /polish|llm|enhanc/.test(processingLabel(bar.phase).toLowerCase())
                      ? "polishing"
                      : "transcribing"
                    : "idle"
              }`}
              // Phase is conveyed by motion + hue, not text — but keep the exact
              // sub-phase (Server transcribing / Using local runtime / Enhancing…)
              // reachable on hover + for a11y so the diagnostic isn't lost.
              title={bar.kind === "processing" ? processingLabel(bar.phase) : bar.kind === "recording" ? (longDictationLocked ? "Long dictation" : "Listening") : undefined}
              aria-label={bar.kind === "processing" ? processingLabel(bar.phase) : bar.kind === "recording" ? (longDictationLocked ? "Long dictation" : "Listening") : undefined}
            >
              {bar.kind === "recording" && polishModeEnabled && (
                <span className="sb-mode-badge sb-mode-badge--recording">Polish</span>
              )}
              {Array.from({ length: 15 }).map((_, index) => (
                <span
                  key={index}
                  style={{
                    transform: `scaleY(${Math.max(WAVE_REST, barTargets.current[index] || WAVE_REST).toFixed(3)})`,
                    opacity: bar.kind === "recording" ? 0.62 + audioLevel * 0.38 : 0.9,
                  }}
                />
              ))}
            </div>
          )}

          {bar.kind === "learned" && bar.wordId !== undefined ? (
            <button
              className="sb-survey-undo"
              title="Undo — remove this word from your dictionary"
              aria-label="Undo"
              onClick={async () => {
                try {
                  await invoke("delete_dictionary_word", { id: bar.wordId });
                } catch (e) {
                  console.warn("[status-bar] delete_dictionary_word failed", e);
                }
                setBar({ kind: "idle" });
                invoke("dismiss_status_bar").catch(() => {});
              }}
            >
              <RotateCcw size={10} />
              <span>Undo</span>
            </button>
          ) : bar.kind === "error" ? (
            <>
              {bar.audioId && (
                <button
                  className="sb-survey-icon-btn sb-survey-icon-btn--primary"
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
                className="sb-survey-icon-btn"
                title="Copy error details"
                aria-label="Copy error details"
                onClick={() => {
                  const details = [
                    bar.message,
                    bar.errorCode ? `code=${bar.errorCode}` : "",
                    bar.runId ? `run_id=${bar.runId}` : "",
                    bar.audioId ? `audio_id=${bar.audioId}` : "",
                    bar.diagnostic || bar.rawError || "",
                  ].filter(Boolean).join("\n");
                  navigator.clipboard.writeText(details).catch((err) => {
                    console.warn("[status-bar] copy error details failed", err);
                  });
                }}
              >
                <Copy size={12} />
              </button>
              {bar.audioId && (
                <button
                  className="sb-survey-icon-btn"
                  title="Show saved audio"
                  aria-label="Show saved audio"
                  onClick={async () => {
                    try {
                      await invoke("reveal_saved_audio", { audioId: bar.audioId });
                    } catch (e) {
                      console.warn("[status-bar] reveal saved audio failed", e);
                    }
                  }}
                >
                  <Download size={12} />
                </button>
              )}
              <button
                className="sb-survey-icon-btn"
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
            </>
          ) : null}
        </div>
      </div>
    </CardHost>
  );
}
