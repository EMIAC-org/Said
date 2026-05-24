import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow, LogicalPosition, LogicalSize, primaryMonitor } from "@tauri-apps/api/window";
import { RotateCcw, X } from "lucide-react";
import type { AppSnapshot } from "./types";

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
  | { kind: "confirming"; term: string; original: string; context: string; recordingId: string }
  | { kind: "negative_confirm"; term: string; wrongReplacement: string }
  | { kind: "wrong_fixed"; term: string; wrongReplacement: string }
  | { kind: "queued"; term: string; remaining: number }
  | { kind: "reviewing"; candidates: ReviewCandidate[]; selected: Set<number>; recordingId: string }
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

const BOTTOM_OFFSET = 64;
const HUD_CANVAS_MIN_WIDTH = 300;

const LEVEL_SHAPE = [0.28, 0.38, 0.52, 0.68, 0.82, 1.0, 0.78, 0.62, 0.78, 1.0, 0.82, 0.68, 0.52, 0.38, 0.28];
const BAR_DECAY = [0.82, 0.84, 0.85, 0.86, 0.87, 0.88, 0.87, 0.86, 0.87, 0.88, 0.87, 0.86, 0.85, 0.84, 0.82];

// ── Helpers ───────────────────────────────────────────────────────────────────

function textWidth(text: string): number {
  return Math.ceil(text.length * 6.8);
}

function pillSize(kind: PillKind, hasTranscript = false, label = "", candidateCount = 0): { width: number; height: number } {
  if (hasTranscript) return { width: 280, height: 96 };
  if (kind === "confirming") return { width: 280, height: 142 };
  if (kind === "negative_confirm") return { width: 280, height: 142 };
  if (kind === "reviewing") {
    const n = candidateCount;
    const contentH = 120 + n * 44;
    return { width: 300, height: Math.min(contentH, 350) };
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

// ── Component ─────────────────────────────────────────────────────────────────

export default function StatusBar() {
  const [bar, setBar] = useState<BarState>({ kind: "idle" });
  const [liveTranscript, setLiveTranscript] = useState("");
  const [audioLevel, setAudioLevel] = useState(0);
  const doneTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const audioLevelRef = useRef(0);
  const barTargets = useRef<number[]>(new Array(15).fill(0));
  const [, forceFrame] = useState(0);
  const win = getCurrentWindow();
  const hasTranscript = bar.kind === "processing" && liveTranscript.trim().length > 0;
  const isInteractive = bar.kind === "confirming" || bar.kind === "negative_confirm" || bar.kind === "reviewing";

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
      case "queued": return `"${bar.term}" — ${bar.remaining === 1 ? "1 more edit to learn" : `${bar.remaining} more edits to learn`}`;
      case "wrong_fixed": return `Got it — won’t type "${bar.wrongReplacement}" for "${bar.term}"`;
      case "retraining": return "Improving model...";
      case "retrain_done": return bar.durationS > 0 ? `Model updated (${bar.durationS.toFixed(1)}s)` : "Model updated";
      default: return "";
    }
  })();

  const candidateCount = bar.kind === "reviewing" ? bar.candidates.length : 0;
  const innerSize = pillSize(bar.kind, hasTranscript, pillLabel, candidateCount);

  useEffect(() => {
    invoke("set_status_bar_interactive", { interactive: isInteractive }).catch(() => {});
  }, [isInteractive]);

  // Resize the native window to fit the current pill/card content.
  useEffect(() => {
    const w = Math.max(innerSize.width + 40, HUD_CANVAS_MIN_WIDTH);
    const h = Math.max(innerSize.height + 40, 56);
    primaryMonitor().then((monitor) => {
      const scale = monitor?.scaleFactor ?? 1;
      const sw = monitor ? monitor.size.width / scale : 1440;
      const sh = monitor ? monitor.size.height / scale : 900;
      const sx = monitor ? monitor.position.x / scale : 0;
      const sy = monitor ? monitor.position.y / scale : 0;
      const x = sx + sw / 2 - w / 2;
      const y = sy + sh - h - BOTTOM_OFFSET;
      win.setSize(new LogicalSize(w, h))
        .then(() => win.setPosition(new LogicalPosition(x, y)))
        .catch(() => {});
    }).catch(() => {});
  }, [innerSize.width, innerSize.height]);

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
    if (bar.kind === "idle") {
      const t = setTimeout(() => {
        invoke("dismiss_status_bar").catch(() => {});
      }, 500);
      return () => clearTimeout(t);
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
        playSound("chimeUp");
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
        // Only auto-hide if we're not waiting on a user-action (error/done/confirm)
        setBar((prev) => {
          if (prev.kind === "error") return prev; // user must dismiss
          if (prev.kind === "confirming" || prev.kind === "negative_confirm") return prev; // user must respond
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

    // ── Learning: show term in status bar with undo ──────────────────
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

  // ── Toast states render as expanded cards, not pills ──────────────────
  if (bar.kind === "confirming") {
    return (
      <div
        className="sb-toast sb-toast-confirm"
        style={{ width: innerSize.width }}
        aria-label="AirNote confirming"
      >
        <div className="sb-toast-header">
          <span className="sb-toast-dot sb-toast-dot-yellow" />
          <span className="sb-toast-title">Quick question</span>
        </div>
        <div className="sb-toast-body">
          You changed <span className="sb-toast-old">{bar.original}</span>
          <span className="sb-toast-arrow"> → </span>
          <span className="sb-toast-term">{bar.term}</span>
          <br />
          Is <strong>"{bar.term}"</strong> a product, brand, or name?
        </div>
        <div className="sb-toast-actions">
          <button className="sb-toast-btn sb-toast-btn-primary" onClick={() => handleConfirm(bar.term, bar.original, "learn", bar.recordingId)}>
            Yes, learn it
          </button>
          <button className="sb-toast-btn sb-toast-btn-secondary" onClick={() => handleConfirm(bar.term, bar.original, "skip", bar.recordingId)}>
            No, just rephrasing
          </button>
        </div>
      </div>
    );
  }

  if (bar.kind === "reviewing") {
    const sel = bar.selected;
    const selCount = sel.size;
    const toggleIdx = (idx: number) => {
      if (!bar.candidates[idx]?.learnable) return;
      setBar((prev) => {
        if (prev.kind !== "reviewing") return prev;
        const next = new Set(prev.selected);
        if (next.has(idx)) next.delete(idx); else next.add(idx);
        return { ...prev, selected: next };
      });
    };
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
      setBar({ kind: "idle" });
      invoke("dismiss_status_bar").catch(() => {});
    };
    return (
      <div className="sb-review-card sb-shell--interactive" style={{ width: innerSize.width, height: innerSize.height }}>
        <div className="sb-review-header">
          <span className="sb-toast-dot sb-toast-dot-yellow" />
          <div>
            <div className="sb-review-title">Which corrections should AirNote learn?</div>
            <div className="sb-review-sub">Tap the ones you want remembered</div>
          </div>
        </div>
        <div className="sb-review-changes">
          {bar.candidates.slice(0, 4).map((c, i) => (
            <div
              key={i}
              className={`sb-review-row${sel.has(i) ? " selected" : ""}${!c.learnable ? " disabled" : ""}`}
              onClick={() => toggleIdx(i)}
            >
              <div className="sb-review-check">{sel.has(i) ? "✓" : ""}</div>
              <div className="sb-review-text">
                <span className="sb-review-from">{c.learnable ? c.original || "(empty)" : "(not in audio)"}</span>
                <span className="sb-review-arrow">&rarr;</span>
                <span className="sb-review-to">{c.corrected}</span>
              </div>
              <span className={`sb-review-tag${c.tag === "added" ? " sb-review-tag-added" : ""}`}>{c.tag}</span>
            </div>
          ))}
        </div>
        <div className="sb-review-footer">
          <button className="sb-review-skip" onClick={handleSkip}>Skip all</button>
          <span className="sb-review-hint">{selCount > 0 ? `${selCount} selected` : "none"}</span>
          <button className="sb-review-learn" onClick={handleLearn} disabled={selCount === 0}>
            {selCount > 0 ? `Learn ${selCount}` : "Learn"}
          </button>
        </div>
      </div>
    );
  }

  if (bar.kind === "negative_confirm") {
    return (
      <div
        className="sb-toast sb-toast-negative"
        style={{ width: innerSize.width }}
        aria-label="AirNote wrong correction"
      >
        <div className="sb-toast-header">
          <span className="sb-toast-dot sb-toast-dot-red" />
          <span className="sb-toast-title">Wrong correction detected</span>
        </div>
        <div className="sb-toast-body">
          AirNote keeps changing <span className="sb-toast-term">{bar.term}</span> to <strong>"{bar.wrongReplacement}"</strong> but you changed it back.
          <br />
          Should I stop this correction?
        </div>
        <div className="sb-toast-actions">
          <button className="sb-toast-btn sb-toast-btn-danger" onClick={() => handleBlock(bar.term, bar.wrongReplacement)}>
            Yes, stop it
          </button>
          <button className="sb-toast-btn sb-toast-btn-secondary" onClick={() => setBar({ kind: "idle" })}>
            It was right this time
          </button>
        </div>
      </div>
    );
  }

  if (bar.kind === "wrong_fixed") {
    return (
      <div className="sb-shell sb-shell--info" style={{ width: innerSize.width, height: innerSize.height }}>
        <span className="sb-toast-dot sb-toast-dot-red" />
        <span className="sb-retrain-label">{pillLabel}</span>
      </div>
    );
  }

  if (bar.kind === "queued") {
    return (
      <div className="sb-shell sb-shell--info" style={{ width: innerSize.width, height: innerSize.height }}>
        <span className="sb-toast-dot sb-toast-dot-yellow" />
        <span className="sb-retrain-label">{pillLabel}</span>
      </div>
    );
  }

  if (bar.kind === "retraining") {
    return (
      <div className="sb-shell sb-shell--info" style={{ width: innerSize.width, height: innerSize.height }}>
        <span className="sb-toast-dot sb-toast-dot-blue" />
        <span className="sb-retrain-label">{pillLabel}</span>
      </div>
    );
  }

  if (bar.kind === "retrain_done") {
    return (
      <div className="sb-shell sb-shell--info" style={{ width: innerSize.width, height: innerSize.height }}>
        <span className="sb-toast-dot sb-toast-dot-green" />
        <span className="sb-retrain-label">{pillLabel}</span>
      </div>
    );
  }

  return (
    <div
      className={`sb-shell sb-shell--${bar.kind}${hasTranscript ? " sb-shell--expanded" : ""}${isInteractive ? " sb-shell--interactive" : ""}`}
      style={{ width: innerSize.width, height: innerSize.height }}
      aria-label={`AirNote ${bar.kind}`}
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
