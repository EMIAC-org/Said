import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { emit, listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow, LogicalPosition, LogicalSize } from "@tauri-apps/api/window";
import { ChevronLeft, ChevronRight, CornerDownLeft, ListChecks, RotateCcw, X } from "lucide-react";
import type { AppSnapshot } from "./types";
import { APPLY_UPDATE_EVENT } from "./lib/autoUpdate";

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
  | { kind: "update_ready"; version: string; message: string }
  | { kind: "retraining" }
  | { kind: "retrain_done"; durationS: number };

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
  const doneTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const audioLevelRef = useRef(0);
  const barTargets = useRef<number[]>(new Array(15).fill(0));
  const lastResizeRef = useRef<{ width: number; height: number } | null>(null);
  const barKindRef = useRef<BarState["kind"]>("idle");
  const [, forceFrame] = useState(0);
  const [win] = useState(() => getCurrentWindow());
  const hasTranscript = bar.kind === "processing" && liveTranscript.trim().length > 0;
  const isInteractive =
    bar.kind === "confirming"
    || bar.kind === "negative_confirm"
    || bar.kind === "reviewing"
    || bar.kind === "error"
    || bar.kind === "learned"
    || bar.kind === "update_ready"
    || bar.kind === "placement";
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
          setBar((prev) => prev.kind === "processing" ? { kind: "idle" } : prev);
        }, 15000);
      } else if (state === "idle") {
        setBar((prev) => {
          if (prev.kind === "error") return prev;
          if (prev.kind === "confirming" || prev.kind === "negative_confirm" || prev.kind === "reviewing") return prev;
          if (prev.kind === "processing") return prev;
          if (prev.kind === "done" || prev.kind === "pasted" || prev.kind === "manual_paste") return prev;
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

    // ── Error: show message + optional retry ──────────────────────────────
    listen<VoiceErrorPayload & { raw_error?: string }>("voice-error", (e) => {
      const { message, audio_id, auto_hide_ms, raw_error } = e.payload;
      console.error("[status-bar] voice-error event", { message, raw_error, hasAudioId: Boolean(audio_id) });
      if (doneTimer.current) clearTimeout(doneTimer.current);
      if (!notifEnabled("error")) return;
      win.show().catch((err) => console.warn("[status-bar] show failed for error", err));
      playSound("lowThud");
      setBar({ kind: "error", message, audioId: audio_id });
      if (typeof auto_hide_ms === "number" && auto_hide_ms > 0) {
        doneTimer.current = setTimeout(() => setBar({ kind: "idle" }), auto_hide_ms);
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
      setBar({ kind: "idle" });
    }).then((fn) => {
      subs.push(fn);
    }).catch((err) => console.warn("[status-bar] placement finish subscribe failed", err));

    // ── Learning notifications ────────────────────────────────────────
    listen<{ term: string; message: string }>("vocab-learned", (e) => {
      if (!notifEnabled("learned")) return;
      console.info("[status-bar] vocab-learned", e.payload);
      if (doneTimer.current) clearTimeout(doneTimer.current);
      win.show().catch(() => {});
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
      win.show().catch(() => {});
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
      win.show().catch(() => {});
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
      win.show().catch(() => {});
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
      win.show().catch(() => {});
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
      win.show().catch(() => {});
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
      win.show().catch(() => {});
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
        win.show().catch(() => {});
        setBar({ kind: "retraining" });
      } else if (e.payload.phase === "done") {
        playSound("shimmer");
        const dur = e.payload.duration_s ?? 0;
        setBar({ kind: "retrain_done", durationS: dur });
        if (doneTimer.current) clearTimeout(doneTimer.current);
        doneTimer.current = setTimeout(() => setBar({ kind: "idle" }), 10000);
      }
    }).then((fn) => {
      subs.push(fn);
    }).catch(() => {});

    listen<{ version: string; message?: string }>("auto-update-ready", (e) => {
      if (!notifEnabled("updates")) return;
      console.info("[status-bar] auto-update-ready", e.payload);
      if (doneTimer.current) clearTimeout(doneTimer.current);
      win.show().catch(() => {});
      playSound("shimmer");
      setBar({
        kind: "update_ready",
        version: e.payload.version,
        message: e.payload.message || `Update ${e.payload.version} downloaded. Restart AirNote to use it.`,
      });
    }).then((fn) => {
      subs.push(fn);
    }).catch(() => {});

    return () => {
      console.info("[status-bar] unmount subscriptions", subs.length);
      subs.forEach((fn) => fn());
    };
  }, []);

  useEffect(() => () => { if (doneTimer.current) clearTimeout(doneTimer.current); }, []);

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
        const n = await invoke<number>("confirm_batch", { items, recordingId: bar.recordingId });
        playSound("levelUp");
        setBar({ kind: "learned", term: `${n} correction${n > 1 ? "s" : ""}`, message: `Learned ${n} correction${n > 1 ? "s" : ""}` });
        if (doneTimer.current) clearTimeout(doneTimer.current);
        doneTimer.current = setTimeout(() => { setBar({ kind: "idle" }); invoke("dismiss_status_bar").catch(() => {}); }, 3000);
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
            <button type="button" className="sb-survey-skip" onClick={() => setBar({ kind: "idle" })}>
              Later
            </button>
            <button type="button" className="sb-survey-next" onClick={() => void emit(APPLY_UPDATE_EVENT)}>
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
