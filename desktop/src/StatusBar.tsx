import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { emit, listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow, LogicalPosition, LogicalSize } from "@tauri-apps/api/window";
import { ChevronLeft, ChevronRight, Copy, CornerDownLeft, ListChecks, Mic, Pencil, Plus, RotateCcw, Send, Sparkles, X } from "lucide-react";
import type { AppSnapshot } from "./types";
import { APPLY_UPDATE_FAILED_EVENT, getPendingReadyUpdateVersion, requestApplyPendingUpdate } from "./lib/autoUpdate";
import { divoListThreads, type DivoThreadSummary } from "./lib/invoke";
import { Markdown } from "./components/Markdown";

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
  | { kind: "manual_paste" }
  | { kind: "error"; message: string; audioId?: string }
  | { kind: "learned"; term: string; message: string }
  | { kind: "email_saved"; email: string; message: string }
  | { kind: "confirming"; term: string; original: string; context: string; recordingId: string }
  | { kind: "negative_confirm"; term: string; wrongReplacement: string }
  | { kind: "wrong_fixed"; term: string; wrongReplacement: string }
  | { kind: "queued"; term: string; remaining: number }
  | { kind: "reviewing"; candidates: ReviewCandidate[]; selected: Set<number>; recordingId: string }
  | { kind: "placement"; message: string }
  | { kind: "polish_mode"; enabled: boolean; message: string }
  | { kind: "update_ready"; version: string; message: string }
  | { kind: "retraining" }
  | { kind: "retrain_done"; durationS: number }
  // ── Divo (Ctrl hold-to-talk → agent) ──
  | { kind: "divo_stage" } // compact review bar: transcript + Send + ✎
  | { kind: "divo_route" } // expanded (on ✎): edit transcript + pick target chat
  | { kind: "divo_streaming" } // live activity panel (status/plan/thinking)
  | { kind: "divo_min" } // hidden — collapsed "Divo is working…" pill
  | { kind: "divo_ready" } // "Divo (1)" notification badge
  | { kind: "divo_answer" } // expanded markdown response panel
  | { kind: "divo_pending"; message: string } // awaiting Lark approval
  | { kind: "divo_error"; message: string };

type DivoTool = { name: string; verb?: string; past?: string; ok?: boolean; done: boolean };
type DivoPlan = { status: string; title: string; subtitle?: string };
/// Where a staged turn will be sent: a brand-new chat, or an existing thread.
type DivoTarget = { type: "new" } | { type: "thread"; id: string; title: string };
type DivoActivity = {
  liveLabel: string;
  progressPct: number;
  plan: DivoPlan[];
  thinking: string;
  tools: DivoTool[];
  threadId: string | null;
  answer: string;
  followup: boolean;
};

const emptyDivo = (followup = false): DivoActivity => ({
  liveLabel: followup ? "Sending follow-up…" : "Sending to Divo…",
  progressPct: 4,
  plan: [],
  thinking: "",
  tools: [],
  threadId: null,
  answer: "",
  followup,
});

type UpdateReadyState = Extract<BarState, { kind: "update_ready" }>;

type ReviewCandidate = {
  original: string;
  corrected: string;
  term_type: string;
  learnable: boolean;
  tag: string;
};

type VoiceErrorPayload = {
  message: string;
  audio_id?: string;
  error_code?: string;
  auto_hide_ms?: number;
};

type PillKind = BarState["kind"];

const HUD_CANVAS_MIN_WIDTH = 300;
const VOICE_COMPACT_WIDTH = 150;
const VOICE_INNER_WIDTH = 280;
const VOICE_COMPACT_HEIGHT = 36;
const VOICE_INNER_HEIGHT = 98;
const VOICE_CANVAS_WIDTH = VOICE_INNER_WIDTH + 40;
const VOICE_CANVAS_HEIGHT = VOICE_INNER_HEIGHT + 40;
const REVIEW_PAGE_SIZE = 5;
const REVIEW_PILL_HEIGHT = 36;
const REVIEW_CARD_WIDTH = 352;
/** Compact survey card — list scrolls inside fixed height. */
const REVIEW_CARD_HEIGHT = 264;
const REVIEW_EXPAND_MS = 160;

const LEVEL_SHAPE = [0.28, 0.38, 0.52, 0.68, 0.82, 1.0, 0.78, 0.62, 0.78, 1.0, 0.82, 0.68, 0.52, 0.38, 0.28];
const BAR_DECAY = [0.82, 0.84, 0.85, 0.86, 0.87, 0.88, 0.87, 0.86, 0.87, 0.88, 0.87, 0.86, 0.85, 0.84, 0.82];

// ── Helpers ───────────────────────────────────────────────────────────────────

function textWidth(text: string): number {
  return Math.ceil(text.length * 6.8);
}

function reviewPillLabel(count: number): string {
  return `${count} correction${count !== 1 ? "s" : ""}`;
}

function reviewLetter(slot: number): string {
  return String.fromCharCode(65 + slot);
}

function reviewTagHint(tag: string): string {
  switch (tag) {
    case "added": return "new word";
    case "case": return "capitalization";
    default: return "swapped";
  }
}

function pillSize(
  kind: PillKind,
  hasTranscript = false,
  label = "",
  candidateCount = 0,
  reviewExpanded = true,
): { width: number; height: number } {
  if (hasTranscript) return { width: VOICE_INNER_WIDTH, height: VOICE_INNER_HEIGHT };
  if (kind === "divo_stage") return { width: 520, height: 58 };
  if (kind === "divo_route") return { width: 460, height: 338 };
  if (kind === "divo_streaming") return { width: 340, height: 236 };
  if (kind === "divo_answer") return { width: 440, height: 468 };
  if (kind === "divo_min") return { width: 188, height: 38 };
  if (kind === "divo_ready") return { width: 168, height: 46 };
  if (kind === "divo_pending") return { width: 300, height: 104 };
  if (kind === "divo_error") return { width: 300, height: 96 };
  if (kind === "confirming") return { width: 280, height: 142 };
  if (kind === "negative_confirm") return { width: 280, height: 142 };
  if (kind === "reviewing") {
    if (!reviewExpanded) {
      const pillLabel = reviewPillLabel(candidateCount);
      const content = textWidth(pillLabel) + 8 + 14 + 18;
      const padded = Math.ceil(content * 1.12);
      return {
        width: Math.max(168, Math.min(padded, 300)),
        height: REVIEW_PILL_HEIGHT,
      };
    }
    return { width: REVIEW_CARD_WIDTH, height: REVIEW_CARD_HEIGHT };
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
  if (p.includes("server_audio_fallback")) return "Using local runtime";
  if (p.includes("server_transcrib") || p.includes("server-audio")) return "Server transcribing";
  if (p.includes("server_polish") || p.includes("server-polish") || p.includes("server_polishing")) return "Server polish";
  if (p.includes("message_polish") || p.includes("message-polish")) return "Polishing message";
  if (p.includes("polish") || p.includes("llm") || p.includes("enhanc")) return "Enhancing";
  if (p.includes("paste")) return "Pasting";
  return "Transcribing";
}

function barHeight(barLevel: number, active: boolean): number {
  if (!active) return 4;
  return 4 + barLevel * 24;
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
  const [reviewPage, setReviewPage] = useState(0);
  const [reviewExpanded, setReviewExpanded] = useState(false);
  const [showAllCandidates, setShowAllCandidates] = useState(false);
  const [dragUnlocked, setDragUnlocked] = useState(false);
  const reviewListRef = useRef<HTMLDivElement | null>(null);
  const dragActiveRef = useRef(false);
  const [liveTranscript, setLiveTranscript] = useState("");
  const [audioLevel, setAudioLevel] = useState(0);
  const [divo, setDivo] = useState<DivoActivity>(() => emptyDivo());
  const [divoCopied, setDivoCopied] = useState(false);
  const [divoDraft, setDivoDraft] = useState("");
  const [divoTarget, setDivoTarget] = useState<DivoTarget>({ type: "new" });
  const [divoThreads, setDivoThreads] = useState<DivoThreadSummary[]>([]);
  const divoDraftRef = useRef<HTMLTextAreaElement | null>(null);
  const doneTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const serverNotifyPendingRef = useRef(false);
  const audioLevelRef = useRef(0);
  const pinnedUpdateRef = useRef<UpdateReadyState | null>(null);
  const barTargets = useRef<number[]>(new Array(15).fill(0));
  const lastResizeRef = useRef<{ width: number; height: number } | null>(null);
  const barKindRef = useRef<BarState["kind"]>("idle");
  const [, forceFrame] = useState(0);
  const [win] = useState(() => getCurrentWindow());
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
  // Release the Divo visibility hold, then return to idle. Clearing first (and
  // awaiting it) ensures the native dismiss isn't blocked by the still-active hold.
  const releaseDivoHoldThenIdle = async (reason: string) => {
    try {
      await invoke("set_status_bar_persistent", { persistent: false, reason });
    } catch { /* ignore */ }
    returnToIdleOrPinned(reason, true);
  };
  const hasTranscript = bar.kind === "processing" && liveTranscript.trim().length > 0;
  const isInteractive =
    bar.kind === "confirming"
    || bar.kind === "negative_confirm"
    || bar.kind === "reviewing"
    || bar.kind === "error"
    || bar.kind === "learned"
    || bar.kind === "update_ready"
    || bar.kind === "placement"
    // Divo: the working HUD stays VISIBLE (persistent hold) but click-through —
    // it floats over the user's app, so making it interactive would swallow every
    // click over its area for the whole run and feel like the app froze. Only the
    // review/staging step (edit + Send), the "Divo (1)" notification, and the
    // opened answer panel grab clicks.
    || bar.kind === "divo_stage"
    || bar.kind === "divo_route"
    || bar.kind === "divo_ready"
    || bar.kind === "divo_answer";
  const isFullBleedCard = bar.kind === "reviewing" && reviewExpanded;

  const pillLabel = (() => {
    switch (bar.kind) {
      case "idle": return "AirNote";
      case "recording": return "Recording";
      case "processing": return bar.phase;
      case "done": return "Done";
      case "pasted": return "Pasted";
      case "manual_paste": return "Pasted";
      case "error": return bar.message;
      case "learned": return bar.message;
      case "email_saved": return bar.message;
      case "queued": return `"${bar.term}" — ${bar.remaining === 1 ? "1 more edit to learn" : `${bar.remaining} more edits to learn`}`;
      case "wrong_fixed": return `Got it — won’t type "${bar.wrongReplacement}" for "${bar.term}"`;
      case "placement": return bar.message;
      case "polish_mode": return bar.message;
      case "update_ready": return `Update ${bar.version} ready`;
      case "retraining": return "Improving model...";
      case "retrain_done": return bar.durationS > 0 ? `Model updated (${bar.durationS.toFixed(1)}s)` : "Model updated";
      default: return "";
    }
  })();

  const candidateCount = bar.kind === "reviewing" ? bar.candidates.length : 0;
  const innerSize = pillSize(bar.kind, hasTranscript, pillLabel, candidateCount, reviewExpanded);

  useEffect(() => {
    barKindRef.current = bar.kind;
  }, [bar.kind]);

  useEffect(() => {
    if (bar.kind === "reviewing") {
      setReviewPage(0);
      setShowAllCandidates(false);
    }
  }, [bar.kind === "reviewing" ? bar.recordingId : null, bar.kind === "reviewing" ? bar.candidates.length : 0]);

  useEffect(() => {
    reviewListRef.current?.scrollTo({ top: 0, behavior: "smooth" });
  }, [reviewPage]);

  // Compact pill first, then expand to the full review card.
  useEffect(() => {
    if (bar.kind !== "reviewing") {
      setReviewExpanded(false);
      return;
    }
    setReviewExpanded(false);
    const t = window.setTimeout(() => setReviewExpanded(true), REVIEW_EXPAND_MS);
    return () => window.clearTimeout(t);
  }, [bar.kind === "reviewing" ? bar.recordingId : null, bar.kind]);

  useEffect(() => {
    const root = document.documentElement;
    const app = document.getElementById("app");
    if (bar.kind !== "idle") {
      root?.classList.add("sb-card-mode");
      if (isFullBleedCard) {
        app?.classList.add("sb-app--card");
      } else {
        app?.classList.remove("sb-app--card");
      }
    } else {
      root?.classList.remove("sb-card-mode");
      app?.classList.remove("sb-app--card");
    }
    return () => {
      root?.classList.remove("sb-card-mode");
      app?.classList.remove("sb-app--card");
    };
  }, [bar.kind, isFullBleedCard]);

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
    const isReviewing = bar.kind === "reviewing";
    const usesVoiceCanvas = bar.kind === "recording" || bar.kind === "processing";
    const w = isReviewing
      ? REVIEW_CARD_WIDTH
      : usesVoiceCanvas
        ? VOICE_CANVAS_WIDTH
        : Math.max(innerSize.width + 40, HUD_CANVAS_MIN_WIDTH);
    const h = isReviewing
      ? REVIEW_CARD_HEIGHT
      : usesVoiceCanvas
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
    if (isReviewing && reviewExpanded) {
      const t = window.setTimeout(apply, 48);
      return () => window.clearTimeout(t);
    }
  }, [innerSize.width, innerSize.height, bar.kind, reviewExpanded, win]);

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
    if (snap.state === "recording") {
      setBar((prev) =>
        prev.kind === "recording"
          ? prev
          : { kind: "recording", startMs: Date.now() },
      );
    } else if (snap.state === "processing") {
      setBar((prev) =>
        prev.kind === "processing"
          ? prev
          : { kind: "processing", phase: "stt" },
      );
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
    void getPendingReadyUpdateVersion().then((version) => {
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
      if (state === "recording") {
        if (barKindRef.current !== "recording") {
          if (doneTimer.current) clearTimeout(doneTimer.current);
          setLiveTranscript("");
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
        setBar((prev) =>
          prev.kind === "recording"
            ? { kind: "processing", phase: "stt" }
            : prev.kind === "processing" ? prev
            : { kind: "processing", phase: "stt" }
        );
        if (doneTimer.current) clearTimeout(doneTimer.current);
        doneTimer.current = setTimeout(() => {
          if (restorePinnedUpdate("auto-update-ready-after-processing-timeout")) return;
          setBar((prev) => prev.kind === "processing" ? { kind: "idle" } : prev);
        }, 15000);
      } else if (state === "idle") {
        if (restorePinnedUpdate("auto-update-ready-idle")) return;
        setBar((prev) => {
          if (prev.kind === "error") return prev;
          if (prev.kind === "confirming" || prev.kind === "negative_confirm" || prev.kind === "reviewing") return prev;
          if (prev.kind === "processing") return prev;
          if (prev.kind === "done" || prev.kind === "pasted" || prev.kind === "manual_paste") return prev;
          if (prev.kind === "update_ready") return prev;
          if (prev.kind.startsWith("divo")) return prev; // Divo flow owns the HUD
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
        prev.kind === "processing"
          ? (prev.phase === phase ? prev : { kind: "processing", phase })
          : prev
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

    listen<{ status: "pasted" | "manual_paste"; message?: string }>("voice-output", (e) => {
      console.info("[status-bar] voice-output event", e.payload);
      if (restorePinnedUpdate("auto-update-ready-after-output")) return;
      if (doneTimer.current) clearTimeout(doneTimer.current);
      playSound("whoosh");
      setBar({ kind: e.payload.status });
      doneTimer.current = setTimeout(
        () => setBar({ kind: "idle" }),
        e.payload.status === "pasted" ? 100 : 5200,
      );
    }).then((fn) => {
      console.info("[status-bar] subscribed voice-output");
      subs.push(fn);
    }).catch((err) => console.warn("[status-bar] voice-output subscribe failed", err));

    listen<{ enabled: boolean; message?: string }>("message-polish-mode", (e) => {
      console.info("[status-bar] message-polish-mode event", e.payload);
      if (restorePinnedUpdate("auto-update-ready-after-polish-mode")) return;
      if (doneTimer.current) clearTimeout(doneTimer.current);
      presentStatusBar("message-polish-mode");
      playSound(e.payload.enabled ? "levelUp" : "tick");
      setBar({
        kind: "polish_mode",
        enabled: e.payload.enabled,
        message: e.payload.message || (e.payload.enabled ? "Polish mode on" : "Polish mode off"),
      });
      doneTimer.current = setTimeout(() => returnToIdleOrPinned("message-polish-mode-hide"), 1700);
    }).then((fn) => {
      console.info("[status-bar] subscribed message-polish-mode");
      subs.push(fn);
    }).catch((err) => console.warn("[status-bar] message-polish-mode subscribe failed", err));

    // ── Error: show message + optional retry ──────────────────────────────
    listen<VoiceErrorPayload & { raw_error?: string }>("voice-error", (e) => {
      const { message, audio_id, auto_hide_ms, raw_error } = e.payload;
      console.error("[status-bar] voice-error event", { message, raw_error, hasAudioId: Boolean(audio_id) });
      if (doneTimer.current) clearTimeout(doneTimer.current);
      if (!notifEnabled("error")) return;
      presentStatusBar("voice-error");
      playSound("lowThud");
      setBar({ kind: "error", message, audioId: audio_id });
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

    listen("long-dictation-locked", () => {
      console.info("[status-bar] long-dictation-locked event");
    }).then((fn) => {
      subs.push(fn);
    }).catch((err) => console.warn("[status-bar] long-dictation subscribe failed", err));

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
    listen<{ term: string; message: string }>("vocab-learned", (e) => {
      serverNotifyPendingRef.current = false; // cancel deferred fallback toast
      if (!notifEnabled("learned")) return;
      console.info("[status-bar] vocab-learned", e.payload);
      if (doneTimer.current) clearTimeout(doneTimer.current);
      presentStatusBar("vocab-learned");
      playSound("levelUp");
      setBar({ kind: "learned", term: e.payload.term, message: e.payload.message });
      doneTimer.current = setTimeout(() => {
        setBar({ kind: "idle" });
        invoke("dismiss_status_bar").catch(() => {});
      }, 3000);
    }).then((fn) => {
      subs.push(fn);
    }).catch(() => {});

    listen<{ email: string; message: string }>("email-learned", (e) => {
      if (!notifEnabled("learned")) return;
      console.info("[status-bar] email-learned", e.payload);
      if (doneTimer.current) clearTimeout(doneTimer.current);
      presentStatusBar("email-learned");
      playSound("levelUp");
      setBar({ kind: "email_saved", email: e.payload.email, message: e.payload.message });
      doneTimer.current = setTimeout(() => {
        setBar({ kind: "idle" });
        invoke("dismiss_status_bar").catch(() => {});
      }, 3000);
    }).then((fn) => {
      subs.push(fn);
    }).catch(() => {});

    // ── Queued term — show remaining edits needed ─────────────────────
    listen<{ term: string; remaining: number }>("vocab-queued", (e) => {
      if (!notifEnabled("queued")) return;
      console.info("[status-bar] vocab-queued", e.payload);
      if (doneTimer.current) clearTimeout(doneTimer.current);
      presentStatusBar("vocab-queued");
      playSound("tick");
      setBar({ kind: "queued", term: e.payload.term, remaining: e.payload.remaining });
      doneTimer.current = setTimeout(() => { setBar({ kind: "idle" }); invoke("dismiss_status_bar").catch(() => {}); }, 5000);
    }).then((fn) => {
      subs.push(fn);
    }).catch(() => {});

    // ── Ambiguous term — needs user confirmation ──────────────────────
    listen<{ term: string; original: string; context: string; recording_id: string }>("vocab-confirm", (e) => {
      if (!notifEnabled("confirm")) return;
      console.info("[status-bar] vocab-confirm", e.payload);
      if (doneTimer.current) clearTimeout(doneTimer.current);
      presentStatusBar("vocab-confirm");
      playSound("knock");
      setBar({ kind: "confirming", term: e.payload.term, original: e.payload.original, context: e.payload.context, recordingId: e.payload.recording_id });
    }).then((fn) => {
      subs.push(fn);
    }).catch(() => {});

    // ── Review card — multi-change edit review ────────────────────────
    listen<{ candidates: ReviewCandidate[]; recording_id: string }>("vocab-review", (e) => {
      if (!notifEnabled("learned")) return;
      console.info("[status-bar] vocab-review", e.payload);
      if (doneTimer.current) clearTimeout(doneTimer.current);
      presentStatusBar("vocab-review");
      playSound("knock");
      const learnable = e.payload.candidates.filter(c => c.learnable);
      const selected = new Set<number>(learnable.map((_, i) => {
        const idx = e.payload.candidates.indexOf(learnable[i]);
        return idx;
      }));
      setBar({
        kind: "reviewing",
        candidates: e.payload.candidates,
        selected,
        recordingId: e.payload.recording_id,
      });
    }).then((fn) => {
      subs.push(fn);
    }).catch(() => {});

    // ── Wrong correction detected (manual block request) ──────────────
    listen<{ term: string; wrong_replacement: string }>("vocab-negative", (e) => {
      if (!notifEnabled("negative")) return;
      console.info("[status-bar] vocab-negative", e.payload);
      if (doneTimer.current) clearTimeout(doneTimer.current);
      presentStatusBar("vocab-negative");
      playSound("alert");
      setBar({ kind: "negative_confirm", term: e.payload.term, wrongReplacement: e.payload.wrong_replacement });
    }).then((fn) => {
      subs.push(fn);
    }).catch(() => {});

    // ── Wrong correction auto-fixed ─────────────────────────────────
    listen<{ term: string; wrong_replacement: string }>("vocab-wrong-fixed", (e) => {
      if (!notifEnabled("negative")) return;
      console.info("[status-bar] vocab-wrong-fixed", e.payload);
      if (doneTimer.current) clearTimeout(doneTimer.current);
      presentStatusBar("vocab-wrong-fixed");
      playSound("chimeDown");
      setBar({ kind: "wrong_fixed", term: e.payload.term, wrongReplacement: e.payload.wrong_replacement });
      doneTimer.current = setTimeout(() => { setBar({ kind: "idle" }); invoke("dismiss_status_bar").catch(() => {}); }, 4000);
    }).then((fn) => {
      subs.push(fn);
    }).catch(() => {});

    // ── Retrain progress ─────────────────────────────────────────────
    listen<{ phase: string; duration_s?: number; success?: boolean }>("retrain-status", (e) => {
      if (!notifEnabled("retrain")) return;
      console.info("[status-bar] retrain-status", e.payload);
      if (e.payload.phase === "started") {
        if (doneTimer.current) clearTimeout(doneTimer.current);
        presentStatusBar("retrain-started");
        setBar({ kind: "retraining" });
      } else if (e.payload.phase === "done") {
        playSound("shimmer");
        const dur = e.payload.duration_s ?? 0;
        setBar({ kind: "retrain_done", durationS: dur });
        if (doneTimer.current) clearTimeout(doneTimer.current);
        doneTimer.current = setTimeout(() => setBar({ kind: "idle" }), 10000);
      } else if (e.payload.phase === "unavailable") {
        // Retrain API not available or feature not enabled on this machine — silent.
        console.info("[status-bar] retrain not available on this machine");
      }
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

    // ── Divo (Ctrl hold-to-talk → agent) ──────────────────────────────────
    // Review step: the polished transcript arrives here for edit + Send, instead
    // of being sent to Divo automatically.
    listen<{ text: string; newChat: boolean; currentThreadId: string | null }>("divo-stage", (e) => {
      console.info("[status-bar] divo-stage", e.payload);
      if (doneTimer.current) clearTimeout(doneTimer.current);
      setDivoCopied(false);
      setDivoDraft(e.payload?.text ?? "");
      // Default routing: Ctrl+N → new chat; otherwise continue the active thread if
      // there is one. The user can override via the ✎ chat router.
      const cur = e.payload?.currentThreadId ?? null;
      const initialTarget: DivoTarget =
        !e.payload?.newChat && cur
          ? { type: "thread", id: cur, title: "Current chat" }
          : { type: "new" };
      setDivoTarget(initialTarget);
      setDivoThreads([]);
      // Hold the HUD open and make it interactive — the compact review bar takes
      // clicks (Send / ✎), and the expanded router edits text + picks a chat.
      invoke("set_status_bar_persistent", { persistent: true, reason: "divo-stage", interactive: true })
        .catch(() => presentStatusBar("divo-stage"));
      setBar({ kind: "divo_stage" });
      // Make the panel key so buttons/textarea can receive input (the status bar is
      // a non-activating panel; without this it accepts clicks but not typing).
      win.setFocus().catch(() => {});
      // Load the chat list for the router, and resolve the active chat's title.
      void divoListThreads().then((threads) => {
        setDivoThreads(threads);
        if (cur) {
          const match = threads.find((t) => t.id === cur);
          if (match?.title) {
            setDivoTarget((prev) =>
              prev.type === "thread" && prev.id === cur
                ? { type: "thread", id: cur, title: match.title }
                : prev,
            );
          }
        }
      });
    }).then((fn) => subs.push(fn)).catch(() => {});

    listen<{ followup: boolean }>("divo-started", (e) => {
      console.info("[status-bar] divo-started", e.payload);
      if (doneTimer.current) clearTimeout(doneTimer.current);
      setDivoCopied(false);
      setDivo(emptyDivo(!!e.payload?.followup));
      // Hold the HUD open for the whole run (the local voice pipeline going idle
      // must not auto-hide it mid-run), but keep it CLICK-THROUGH: it floats over
      // the user's app, so it must never swallow clicks while Divo works.
      invoke("set_status_bar_persistent", { persistent: true, reason: "divo-started", interactive: false })
        .catch(() => presentStatusBar("divo-started"));
      setBar({ kind: "divo_streaming" });
    }).then((fn) => subs.push(fn)).catch(() => {});

    listen<{ threadId: string }>("divo-meta", (e) => {
      setDivo((d) => ({ ...d, threadId: e.payload?.threadId ?? d.threadId }));
    }).then((fn) => subs.push(fn)).catch(() => {});

    listen<{ liveLabel?: string; progressPct?: number; phase?: string; plan?: DivoPlan[] }>(
      "divo-status",
      (e) => {
        const p = e.payload || {};
        setDivo((d) => ({
          ...d,
          liveLabel: p.liveLabel ?? d.liveLabel,
          progressPct: typeof p.progressPct === "number" ? p.progressPct : d.progressPct,
          plan: Array.isArray(p.plan) && p.plan.length ? p.plan : d.plan,
        }));
      },
    ).then((fn) => subs.push(fn)).catch(() => {});

    listen<{ text: string }>("divo-thinking", (e) => {
      const t = e.payload?.text;
      if (t) setDivo((d) => ({ ...d, thinking: t }));
    }).then((fn) => subs.push(fn)).catch(() => {});

    listen<{ phase: "start" | "end"; name: string; verb?: string | null; past?: string | null; ok?: boolean }>(
      "divo-tool",
      (e) => {
        const p = e.payload;
        if (!p) return;
        setDivo((d) => {
          let tools = d.tools.slice();
          if (p.phase === "start") {
            tools.push({ name: p.name, verb: p.verb ?? undefined, done: false });
          } else {
            for (let i = tools.length - 1; i >= 0; i--) {
              if (!tools[i].done) {
                tools[i] = { ...tools[i], done: true, ok: p.ok, past: p.past ?? undefined };
                break;
              }
            }
          }
          if (tools.length > 4) tools = tools.slice(tools.length - 4);
          return { ...d, tools };
        });
      },
    ).then((fn) => subs.push(fn)).catch(() => {});

    listen<{ content: string; threadId: string | null }>("divo-done", (e) => {
      console.info("[status-bar] divo-done");
      if (doneTimer.current) clearTimeout(doneTimer.current);
      const content = e.payload?.content ?? "";
      setDivo((d) => ({
        ...d,
        answer: content,
        threadId: e.payload?.threadId ?? d.threadId,
        progressPct: 100,
      }));
      playSound("ding");
      // Keep the hold open and make it actionable — the "Divo (1)" badge is a
      // click target, so the panel must grab clicks now.
      invoke("set_status_bar_persistent", { persistent: true, reason: "divo-done", interactive: true })
        .catch(() => presentStatusBar("divo-done"));
      setBar({ kind: "divo_ready" });
    }).then((fn) => subs.push(fn)).catch(() => {});

    listen<{ message: string }>("divo-error", (e) => {
      console.error("[status-bar] divo-error", e.payload);
      if (doneTimer.current) clearTimeout(doneTimer.current);
      playSound("lowThud");
      presentStatusBar("divo-error");
      setBar({ kind: "divo_error", message: e.payload?.message || "Divo failed" });
      doneTimer.current = setTimeout(() => { releaseDivoHoldThenIdle("divo-error-hide"); }, 6000);
    }).then((fn) => subs.push(fn)).catch(() => {});

    listen<{ message: string }>("divo-pending", (e) => {
      console.info("[status-bar] divo-pending", e.payload);
      if (doneTimer.current) clearTimeout(doneTimer.current);
      presentStatusBar("divo-pending");
      setBar({ kind: "divo_pending", message: e.payload?.message || "Pending approval in Lark" });
      doneTimer.current = setTimeout(() => { releaseDivoHoldThenIdle("divo-pending-hide"); }, 8000);
    }).then((fn) => subs.push(fn)).catch(() => {});

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

  function dismissToIdle() {
    setBar({ kind: "idle" });
    if (doneTimer.current) clearTimeout(doneTimer.current);
    doneTimer.current = setTimeout(() => {
      invoke("dismiss_status_bar").catch(() => {});
    }, 1500);
  }

  async function handleConfirm(term: string, original: string, action: "learn" | "skip", recordingId: string) {
    try {
      await invoke("confirm_term", { term, original, action, recordingId: recordingId || null });
    } catch (e) {
      console.warn("[status-bar] confirm_term failed:", e);
    }
    if (action === "learn") {
      setBar({ kind: "learned", term, message: `Will recognise "${term}" next time` });
      if (doneTimer.current) clearTimeout(doneTimer.current);
      doneTimer.current = setTimeout(() => dismissToIdle(), 3000);
    } else {
      dismissToIdle();
    }
  }

  async function handleBlock(variant: string, wrongReplacement: string) {
    try {
      await invoke("block_correction", { variant, wrongReplacement });
    } catch (e) {
      console.warn("[status-bar] block_correction failed:", e);
    }
    dismissToIdle();
  }

  // ── Idle: nothing visible; native window hides via dismiss_status_bar ──
  if (bar.kind === "idle") {
    return null;
  }

  // ── Divo: compact review bar — transcript + Send + ✎ (stays horizontal) ──
  if (bar.kind === "divo_stage" || bar.kind === "divo_route") {
    const targetLabel = divoTarget.type === "new" ? "New chat" : (divoTarget.title || "Current chat");
    const sendDraft = () => {
      const text = divoDraft.trim();
      if (!text) return;
      // divo-started (emitted by the send) transitions the HUD to streaming.
      const threadId = divoTarget.type === "new" ? null : divoTarget.id;
      invoke("divo_send", { message: text, threadId }).catch(() => {});
    };

    // Compact horizontal bar — the default. The editor never auto-expands.
    if (bar.kind === "divo_stage") {
      return (
        <CardHost>
          <div
            className="sb-survey sb-survey--interactive divo-review"
            style={{ width: innerSize.width, height: innerSize.height }}
            aria-label="Review instruction before sending to Divo"
            onKeyDown={(e) => {
              if ((e.metaKey || e.ctrlKey) && e.key === "Enter") { e.preventDefault(); sendDraft(); }
              else if (e.key === "Escape") { e.preventDefault(); releaseDivoHoldThenIdle("divo-stage-cancel"); }
            }}
            tabIndex={0}
          >
            <span className="divo-mark"><Sparkles size={11} strokeWidth={2.4} /></span>
            <span className="divo-review-text" title={divoDraft}>{divoDraft || "…"}</span>
            <button
              type="button"
              className={`divo-review-target ${divoTarget.type === "new" ? "is-new" : ""}`}
              title="Choose which chat this goes to"
              onClick={() => setBar({ kind: "divo_route" })}
            >
              {divoTarget.type === "new" ? <Plus size={11} strokeWidth={2.6} /> : null}
              <span className="divo-review-target-nm">{targetLabel}</span>
            </button>
            <button
              type="button"
              className="divo-review-edit"
              title="Edit & route"
              aria-label="Edit and route"
              onClick={() => setBar({ kind: "divo_route" })}
            >
              <Pencil size={13} strokeWidth={2.1} />
            </button>
            <button type="button" className="divo-review-send" onClick={sendDraft} disabled={!divoDraft.trim()}>
              <Send size={12} strokeWidth={2.2} /> Send
            </button>
          </div>
        </CardHost>
      );
    }

    // Expanded editor + tabular chat router — only on ✎.
    return (
      <CardHost>
        <div
          className="sb-survey sb-survey--panel sb-survey--interactive divo-route"
          style={{ width: innerSize.width, height: innerSize.height }}
          aria-label="Edit instruction and choose a chat"
        >
          <div className="divo-head">
            <span className="divo-mark"><Sparkles size={11} strokeWidth={2.4} /></span>
            <span className="divo-name">Edit &amp; route</span>
            <button className="divo-hide" title="Back" aria-label="Back" onClick={() => setBar({ kind: "divo_stage" })}>
              <ChevronLeft size={14} />
            </button>
          </div>
          <textarea
            ref={divoDraftRef}
            className="divo-stage-input"
            value={divoDraft}
            onChange={(e) => setDivoDraft(e.target.value)}
            onKeyDown={(e) => {
              if ((e.metaKey || e.ctrlKey) && e.key === "Enter") { e.preventDefault(); sendDraft(); }
              else if (e.key === "Escape") { e.preventDefault(); setBar({ kind: "divo_stage" }); }
            }}
            spellCheck={false}
            autoFocus
          />
          <div className="divo-route-label">Send to</div>
          <div className="divo-route-chips">
            {divoThreads.map((t) => (
              <button
                key={t.id}
                type="button"
                className={`divo-chip ${divoTarget.type === "thread" && divoTarget.id === t.id ? "active" : ""}`}
                title={t.title}
                onClick={() => setDivoTarget({ type: "thread", id: t.id, title: t.title || "Chat" })}
              >
                {t.title || "Untitled chat"}
              </button>
            ))}
            <button
              type="button"
              className={`divo-chip divo-chip--new ${divoTarget.type === "new" ? "active" : ""}`}
              onClick={() => setDivoTarget({ type: "new" })}
            >
              <Plus size={11} strokeWidth={2.6} /> New chat
            </button>
          </div>
          <div className="divo-stage-foot">
            <span className="divo-route-hint">⌘↵ to send</span>
            <button type="button" className="sb-survey-skip" onClick={() => setBar({ kind: "divo_stage" })}>
              Cancel
            </button>
            <button type="button" className="divo-stage-send" onClick={sendDraft} disabled={!divoDraft.trim()}>
              <Send size={12} strokeWidth={2.2} /> Send
            </button>
          </div>
        </div>
      </CardHost>
    );
  }

  // ── Divo: hidden mini pill / live activity panel ──────────────────────
  if (bar.kind === "divo_min") {
    return (
      <CardHost>
        <div
          className="sb-survey sb-survey--toast sb-survey--interactive divo-min"
          style={{ width: innerSize.width, height: innerSize.height }}
          role="button"
          onClick={() => {
            presentStatusBar("divo-reexpand");
            setBar({ kind: "divo_streaming" });
          }}
        >
          <span className="divo-min-spin" aria-hidden="true" />
          <span className="divo-min-txt">Divo is working…</span>
        </div>
      </CardHost>
    );
  }

  if (bar.kind === "divo_streaming") {
    const pct = Math.max(2, Math.min(100, divo.progressPct));
    const showTools = divo.plan.length === 0 && divo.tools.length > 0;
    return (
      <CardHost>
        <div
          className="sb-survey sb-survey--panel sb-survey--interactive"
          style={{ width: innerSize.width, height: innerSize.height }}
          aria-label="Divo working"
        >
          <div className="divo-head">
            <span className="divo-mark"><Sparkles size={11} strokeWidth={2.4} /></span>
            <span className="divo-name">Divo</span>
            <span className="divo-live">{divo.liveLabel}</span>
            <button
              className="divo-hide"
              title="Hide — Divo keeps working"
              aria-label="Hide"
              onClick={() => setBar({ kind: "divo_min" })}
            >
              <X size={13} />
            </button>
          </div>
          <div className="divo-progress"><i style={{ width: `${pct}%` }} /></div>
          {divo.plan.length > 0 ? (
            <div className="divo-plan">
              {divo.plan.slice(0, 4).map((p, i) => (
                <div key={i} className={`divo-plan-row ${p.status}`}>
                  <span className="divo-plan-ic">{p.status === "done" ? "✓" : ""}</span>
                  <span className="divo-plan-title">{p.title}</span>
                </div>
              ))}
            </div>
          ) : showTools ? (
            <div className="divo-plan">
              {divo.tools.map((t, i) => (
                <div key={i} className={`divo-plan-row ${t.done ? "done" : "running"}`}>
                  <span className="divo-plan-ic">{t.done ? "✓" : ""}</span>
                  <span className="divo-plan-title">
                    {t.done ? t.past || `${t.name} done` : t.verb || `Using ${t.name}…`}
                  </span>
                </div>
              ))}
            </div>
          ) : null}
          {divo.thinking ? <div className="divo-think">{divo.thinking}</div> : null}
        </div>
      </CardHost>
    );
  }

  if (bar.kind === "divo_ready") {
    return (
      <CardHost>
        <div
          className="sb-survey sb-survey--toast sb-survey--interactive divo-badge"
          style={{ width: innerSize.width, height: innerSize.height }}
          role="button"
          onClick={() => {
            presentStatusBar("divo-open-answer");
            setBar({ kind: "divo_answer" });
          }}
        >
          <span className="divo-mark"><Sparkles size={11} strokeWidth={2.4} /></span>
          <span className="divo-badge-txt">Divo</span>
          <span className="divo-badge-count">1</span>
          <ChevronRight size={14} className="divo-badge-chev" />
        </div>
      </CardHost>
    );
  }

  if (bar.kind === "divo_answer") {
    return (
      <CardHost variant="card">
        <div
          className="sb-survey sb-survey--interactive divo-answer"
          style={{ width: innerSize.width, height: innerSize.height }}
          aria-label="Divo answer"
        >
          <div className="divo-panel-head">
            <span className="divo-mark"><Sparkles size={11} strokeWidth={2.4} /></span>
            <div className="divo-panel-titles">
              <div className="divo-panel-title">Divo</div>
              {divo.threadId ? (
                <div className="divo-panel-thread">thread {divo.threadId.slice(0, 8)}</div>
              ) : null}
            </div>
            <button
              className="divo-x"
              title="Close"
              aria-label="Close"
              onClick={() => { releaseDivoHoldThenIdle("divo-close"); }}
            >
              <X size={14} />
            </button>
          </div>
          <div className="divo-panel-body">
            <Markdown content={divo.answer} />
          </div>
          <div className="divo-panel-foot">
            <button
              className="divo-followup"
              title="Hold to speak a follow-up"
              onMouseDown={() => { invoke("divo_followup_begin").catch(() => {}); }}
              onMouseUp={() => { invoke("divo_followup_end").catch(() => {}); }}
              onMouseLeave={() => { invoke("divo_followup_end").catch(() => {}); }}
            >
              <Mic size={14} strokeWidth={2} /> Hold to follow up
            </button>
            <button
              className="divo-copy"
              title="Copy answer"
              onClick={() => {
                navigator.clipboard
                  .writeText(divo.answer)
                  .then(() => setDivoCopied(true))
                  .catch(() => {});
              }}
            >
              <Copy size={13} strokeWidth={2} /> {divoCopied ? "Copied" : "Copy"}
            </button>
          </div>
        </div>
      </CardHost>
    );
  }

  if (bar.kind === "divo_pending") {
    return (
      <CardHost>
        <div
          className="sb-survey sb-survey--panel sb-survey--interactive"
          style={{ width: innerSize.width, height: innerSize.height }}
          aria-label="Divo pending approval"
        >
          <div className="sb-survey-kicker-row">
            <span className="sb-status-dot sb-status-dot--warn" />
            <span className="sb-survey-kicker">Pending approval</span>
          </div>
          <div className="sb-survey-body">{bar.message}</div>
        </div>
      </CardHost>
    );
  }

  if (bar.kind === "divo_error") {
    return (
      <CardHost>
        <div
          className="sb-survey sb-survey--panel sb-survey--interactive"
          style={{ width: innerSize.width, height: innerSize.height }}
          aria-label="Divo error"
        >
          <div className="sb-survey-kicker-row">
            <span className="sb-status-dot sb-status-dot--err" />
            <span className="sb-survey-kicker">Divo</span>
          </div>
          <div className="sb-survey-body">{bar.message}</div>
        </div>
      </CardHost>
    );
  }

  // ── Toast states render as expanded cards, not pills ──────────────────
  if (bar.kind === "confirming") {
    return (
      <CardHost>
        <div
          className="sb-survey sb-survey--panel sb-survey--interactive"
          style={{ width: innerSize.width, height: innerSize.height }}
          aria-label="AirNote confirming"
        >
          <div className="sb-survey-kicker-row">
            <span className="sb-status-dot sb-status-dot--warn" />
            <span className="sb-survey-kicker">Quick question</span>
          </div>
          <div className="sb-survey-body">
            You changed <span className="sb-survey-strike">{bar.original}</span>
            {" → "}
            <span className="sb-survey-chip">{bar.term}</span>
            <br />
            Is <strong>&ldquo;{bar.term}&rdquo;</strong> a product, brand, or name?
          </div>
          <div className="sb-survey-footer">
            <button type="button" className="sb-survey-skip" onClick={() => handleConfirm(bar.term, bar.original, "skip", bar.recordingId)}>
              No, just rephrasing
            </button>
            <button type="button" className="sb-survey-next" onClick={() => handleConfirm(bar.term, bar.original, "learn", bar.recordingId)}>
              Yes, learn it
              <CornerDownLeft size={14} strokeWidth={2} aria-hidden="true" />
            </button>
          </div>
        </div>
      </CardHost>
    );
  }

  if (bar.kind === "reviewing") {
    const sel = bar.selected;
    const selCount = sel.size;
    const learnableTotal = bar.candidates.filter((c) => c.learnable).length;
    const learnableEntries = bar.candidates
      .map((c, i) => ({ c, i }))
      .filter(({ c }) => c.learnable);
    const displayEntries = showAllCandidates
      ? learnableEntries
      : learnableEntries.filter(({ i }) => sel.has(i));
    const totalPages = Math.max(1, Math.ceil(displayEntries.length / REVIEW_PAGE_SIZE));
    const page = Math.min(reviewPage, totalPages - 1);
    const pageStart = page * REVIEW_PAGE_SIZE;
    const pageItems = displayEntries.slice(pageStart, pageStart + REVIEW_PAGE_SIZE);
    const hiddenCount = learnableTotal - selCount;

    const toggleIdx = (idx: number) => {
      if (!bar.candidates[idx]?.learnable) return;
      setBar((prev) => {
        if (prev.kind !== "reviewing") return prev;
        const next = new Set(prev.selected);
        if (next.has(idx)) next.delete(idx); else next.add(idx);
        return { ...prev, selected: next };
      });
    };
    const isLastPage = page >= totalPages - 1;

    const handleLearn = async () => {
      const items = bar.candidates
        .filter((_, i) => sel.has(i))
        .map((c) => ({ original: c.original, corrected: c.corrected }));
      if (items.length === 0) return;
      try {
        const result = await invoke<{ learned_count: number; server_owned?: boolean }>("confirm_batch", { items, recordingId: bar.recordingId });
        const n = result.learned_count;
        if (result.server_owned) {
          // Server-owned: defer toast and let the WS vocab-learned notification take over.
          // Fallback after 1.5s if WS event does not arrive.
          serverNotifyPendingRef.current = true;
          if (doneTimer.current) clearTimeout(doneTimer.current);
          doneTimer.current = setTimeout(() => {
            if (!serverNotifyPendingRef.current) return;
            serverNotifyPendingRef.current = false;
            if (n > 0) {
              playSound("levelUp");
              setBar({ kind: "learned", term: `${n} correction${n > 1 ? "s" : ""}`, message: `Saved ${n} correction${n > 1 ? "s" : ""}` });
              doneTimer.current = setTimeout(() => { setBar({ kind: "idle" }); invoke("dismiss_status_bar").catch(() => {}); }, 3000);
            } else {
              setBar({ kind: "idle" });
              invoke("dismiss_status_bar").catch(() => {});
            }
          }, 1500);
        } else {
          playSound("levelUp");
          setBar({ kind: "learned", term: `${n} correction${n > 1 ? "s" : ""}`, message: `Learned ${n} correction${n > 1 ? "s" : ""}` });
          if (doneTimer.current) clearTimeout(doneTimer.current);
          doneTimer.current = setTimeout(() => { setBar({ kind: "idle" }); invoke("dismiss_status_bar").catch(() => {}); }, 3000);
        }
      } catch (e) { console.error("[review] confirm_batch failed", e); }
    };
    const handleSkip = () => {
      setReviewPage(0);
      setBar({ kind: "idle" });
      invoke("dismiss_status_bar").catch(() => {});
    };
    const handleNext = () => {
      if (!isLastPage) {
        setReviewPage((p) => Math.min(totalPages - 1, p + 1));
        return;
      }
      void handleLearn();
    };
    const pillText = reviewPillLabel(selCount);
    const pillW = innerSize.width;

    return (
      <CardHost variant="card">
        {!reviewExpanded ? (
          <div
            className="sb-survey sb-survey--compact sb-survey--pill sb-survey--interactive"
            style={{ width: pillW, height: REVIEW_PILL_HEIGHT }}
            aria-label="AirNote review"
          >
            <ListChecks size={14} strokeWidth={2} className="sb-review-icon" aria-hidden="true" />
            <span className="sb-survey-kicker sb-survey-em">{pillText}</span>
          </div>
        ) : (
        <div className="sb-survey sb-survey--interactive sb-survey--expanded">
          <div className="sb-survey-top">
            <div className="sb-survey-kicker-row">
              <span className="sb-survey-kicker">
                {selCount} selected{learnableTotal !== selCount ? ` · ${learnableTotal} total` : ""}
              </span>
              {!showAllCandidates && hiddenCount > 0 && (
                <button
                  type="button"
                  className="sb-survey-edit"
                  onClick={() => { setShowAllCandidates(true); setReviewPage(0); }}
                >
                  +{hiddenCount} more
                </button>
              )}
              {showAllCandidates && learnableTotal > selCount && (
                <button
                  type="button"
                  className="sb-survey-edit"
                  onClick={() => { setShowAllCandidates(false); setReviewPage(0); }}
                >
                  Show selected
                </button>
              )}
            </div>
            {totalPages > 1 && (
              <div className="sb-survey-pager">
                <button
                  type="button"
                  className="sb-survey-pager-btn"
                  disabled={page === 0}
                  onClick={() => setReviewPage((p) => Math.max(0, p - 1))}
                  aria-label="Previous page"
                >
                  <ChevronLeft size={14} strokeWidth={1.75} />
                </button>
                <span className="sb-survey-pager-label">{page + 1} of {totalPages}</span>
                <button
                  type="button"
                  className="sb-survey-pager-btn"
                  disabled={page >= totalPages - 1}
                  onClick={() => setReviewPage((p) => Math.min(totalPages - 1, p + 1))}
                  aria-label="Next page"
                >
                  <ChevronRight size={14} strokeWidth={1.75} />
                </button>
              </div>
            )}
          </div>

          <div className="sb-survey-question">
            {showAllCandidates ? "Tap to include or exclude" : "These will be learned"}
          </div>

          <div className="sb-survey-list" ref={reviewListRef}>
            {pageItems.length === 0 ? (
              <div className="sb-survey-empty">
                Nothing selected
                {hiddenCount > 0 && (
                  <button
                    type="button"
                    className="sb-survey-edit"
                    onClick={() => setShowAllCandidates(true)}
                  >
                    Pick corrections
                  </button>
                )}
              </div>
            ) : pageItems.map(({ c, i }, slot) => {
              const selected = sel.has(i);
              return (
                <div
                  key={`${c.original}-${c.corrected}-${i}`}
                  className={`sb-survey-row${selected ? " selected" : ""}`}
                  onClick={() => toggleIdx(i)}
                  role="button"
                  tabIndex={0}
                  onKeyDown={(e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); toggleIdx(i); } }}
                >
                  <span className="sb-survey-letter">{reviewLetter(slot)}</span>
                  <p className="sb-survey-copy">
                    <span className="sb-survey-primary">{c.corrected}</span>
                    <span className="sb-survey-desc">
                      {" "}— was “{c.original || "—"}”
                      {" · "}{reviewTagHint(c.tag)}
                    </span>
                  </p>
                </div>
              );
            })}
          </div>

          <div className="sb-survey-footer">
            <button type="button" className="sb-survey-skip" onClick={handleSkip}>Skip</button>
            <button
              type="button"
              className="sb-survey-next"
              onClick={handleNext}
              disabled={isLastPage && selCount === 0}
            >
              {isLastPage ? (selCount > 0 ? `Learn ${selCount}` : "Learn") : "Next"}
              <CornerDownLeft size={14} strokeWidth={2} aria-hidden="true" />
            </button>
          </div>
        </div>
        )}
      </CardHost>
    );
  }

  if (bar.kind === "negative_confirm") {
    return (
      <CardHost>
        <div
          className="sb-survey sb-survey--panel sb-survey--interactive"
          style={{ width: innerSize.width, height: innerSize.height }}
          aria-label="AirNote wrong correction"
        >
          <div className="sb-survey-kicker-row">
            <span className="sb-status-dot sb-status-dot--err" />
            <span className="sb-survey-kicker">Wrong correction detected</span>
          </div>
          <div className="sb-survey-body">
            AirNote keeps changing <span className="sb-survey-chip">{bar.term}</span> to{" "}
            <strong>&ldquo;{bar.wrongReplacement}&rdquo;</strong> but you changed it back.
            <br />
            Should I stop this correction?
          </div>
          <div className="sb-survey-footer">
            <button type="button" className="sb-survey-skip" onClick={() => setBar({ kind: "idle" })}>
              It was right this time
            </button>
            <button type="button" className="sb-survey-next" onClick={() => handleBlock(bar.term, bar.wrongReplacement)}>
              Yes, stop it
              <CornerDownLeft size={14} strokeWidth={2} aria-hidden="true" />
            </button>
          </div>
        </div>
      </CardHost>
    );
  }

  if (bar.kind === "wrong_fixed") {
    return (
      <CardHost>
        <div
          className="sb-survey sb-survey--compact sb-survey--toast"
          style={{ width: innerSize.width, height: innerSize.height }}
        >
          <span className="sb-status-dot sb-status-dot--ok" />
          <span className="sb-survey-label">{pillLabel}</span>
        </div>
      </CardHost>
    );
  }

  if (bar.kind === "update_ready") {
    return (
      <CardHost>
        <div
          className="sb-survey sb-survey--panel sb-survey--interactive"
          style={{ width: 300, height: 122 }}
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

  if (bar.kind === "queued") {
    return (
      <CardHost>
        <div
          className="sb-survey sb-survey--compact sb-survey--toast"
          style={{ width: innerSize.width, height: innerSize.height }}
        >
          <span className="sb-status-dot sb-status-dot--warn" />
          <span className="sb-survey-label">{pillLabel}</span>
        </div>
      </CardHost>
    );
  }

  if (bar.kind === "retraining") {
    return (
      <CardHost>
        <div
          className="sb-survey sb-survey--compact sb-survey--toast"
          style={{ width: innerSize.width, height: innerSize.height }}
        >
          <span className="sb-status-dot sb-status-dot--info" />
          <span className="sb-survey-label">{pillLabel}</span>
        </div>
      </CardHost>
    );
  }

  if (bar.kind === "retrain_done") {
    return (
      <CardHost>
        <div
          className="sb-survey sb-survey--compact sb-survey--toast"
          style={{ width: innerSize.width, height: innerSize.height }}
        >
          <span className="sb-status-dot sb-status-dot--ok" />
          <span className="sb-survey-label">{pillLabel}</span>
        </div>
      </CardHost>
    );
  }

  const usesVoiceCanvas = bar.kind === "recording" || bar.kind === "processing";
  const voiceSurfaceWidth = hasTranscript ? VOICE_INNER_WIDTH : VOICE_COMPACT_WIDTH;
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

        <div className="sb-survey-controlbar">
          {bar.kind === "processing" ? (
            <div className="sb-survey-processing">
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
              <strong>{bar.term}</strong> learned
            </div>
          ) : bar.kind === "email_saved" ? (
            <div className="sb-survey-label">
              <span className="sb-status-dot sb-status-dot--ok" />
              <strong>{bar.email}</strong> saved
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
            <div className={`sb-survey-visualizer${bar.kind === "recording" ? " sb-survey-visualizer--active" : ""}`}>
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

          {bar.kind === "learned" ? (
            <button
              className="sb-survey-undo"
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
