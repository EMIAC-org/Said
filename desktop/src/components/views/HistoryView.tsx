import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Clock, Copy, Play, Pause, Trash2, MoreHorizontal, Check, Search, X, Download,
  RefreshCw, Monitor, ChevronDown, Undo2, AlertTriangle, FileDown,
} from "lucide-react";
import { ScrollArea } from "@/components/ui/scroll-area";
import type { Recording } from "@/types";
import {
  deleteRecording,
  listHistory,
  getAppIcon,
  downloadRecordingAudio as saveRecordingAudio,
  getRecordingAudioBytes,
  exportHistory,
  revealDownloadedFile,
} from "@/lib/invoke";
import { friendlyError } from "@/lib/friendlyError";

// App-icon cache shared across all rows: app_key → data URL (or null miss).
// Dedups in-flight lookups so N rows pasted into the same app cost one backend
// call total, and survives re-renders/scroll without re-fetching.
const appIconCache = new Map<string, string | null>();
const appIconInflight = new Map<string, Promise<string | null>>();
function resolveAppIcon(appKey: string): Promise<string | null> {
  if (appIconCache.has(appKey)) return Promise.resolve(appIconCache.get(appKey) ?? null);
  const inflight = appIconInflight.get(appKey);
  if (inflight) return inflight;
  const p = getAppIcon(appKey).then((url) => {
    appIconCache.set(appKey, url);
    appIconInflight.delete(appKey);
    return url;
  });
  appIconInflight.set(appKey, p);
  return p;
}

// ── Formatting helpers ────────────────────────────────────────────────────────

/** Duration in "m:ss" (e.g. 0:03, 1:07). Empty for missing/zero. */
function formatDuration(seconds: number | null | undefined): string {
  if (!seconds || seconds <= 0) return "";
  const s = Math.round(seconds);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

/** Friendly model label. Maps known engines; else prettifies the raw id. */
function formatModel(model: string | null | undefined): string {
  if (!model) return "";
  const m = model.toLowerCase();
  if (m.includes("apex")) return "AirNote Native";
  if (m.includes("oriserve")) return "Hinglish (Oriserve)";
  if (m.includes("nova") || m.includes("deepgram")) return "Deepgram";
  if (m.includes("whisper")) return "Whisper";
  if (m.includes("groq") || m.includes("llama")) return "Groq";
  if (m.includes("sarvam")) return "Sarvam";
  return model.length <= 24 ? model : model.slice(0, 22) + "…";
}

/** The app the text was typed into — prettify a bundle id if that's what we got. */
function formatSourceApp(target: string | null | undefined): string {
  if (!target || !target.trim()) return "This device";
  const t = target.trim();
  if (t.includes(" ") || !t.includes(".")) return t; // already a friendly name
  const seg = t.split(".").pop() ?? t;               // com.tinyspeck.slack → slack
  return seg.charAt(0).toUpperCase() + seg.slice(1);
}

/** Build a friendly WAV filename: "airnote-2026-05-03-1430-12-words.wav". */
function audioFilename(recording: Recording): string {
  const d = new Date(recording.timestamp_ms);
  const pad = (n: number) => String(n).padStart(2, "0");
  const stamp = `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}-${pad(d.getHours())}${pad(d.getMinutes())}`;
  return `airnote-${stamp}-${recording.word_count}-words.wav`;
}

async function downloadRecordingAudio(recording: Recording): Promise<string | null> {
  return await saveRecordingAudio(recording.id, audioFilename(recording));
}

/** The "original" (pre-polish) text — enriched STT if present, else raw transcript. */
function originalText(r: Recording): string {
  return (r.enriched_transcript ?? r.transcript ?? "").trim();
}

/** Group rows by calendar day, newest bucket first (preserves ids for actions). */
function groupRecordingsByDay(recordings: Recording[]): { label: string; items: Recording[] }[] {
  if (recordings.length === 0) return [];
  const startOfToday = new Date();
  startOfToday.setHours(0, 0, 0, 0);
  const todayMs = startOfToday.getTime();
  const yesterdayMs = todayMs - 86_400_000;

  const buckets = new Map<string, Recording[]>();
  const order: string[] = [];
  for (const rec of recordings) {
    const startOfItemDay = new Date(rec.timestamp_ms);
    startOfItemDay.setHours(0, 0, 0, 0);
    const itemDayMs = startOfItemDay.getTime();
    const label =
      itemDayMs >= todayMs ? "Today"
      : itemDayMs >= yesterdayMs ? "Yesterday"
      : new Date(rec.timestamp_ms).toLocaleDateString("en-US", { weekday: "long", month: "long", day: "numeric" });
    if (!buckets.has(label)) { buckets.set(label, []); order.push(label); }
    buckets.get(label)!.push(rec);
  }
  return order.map((label) => ({ label, items: buckets.get(label)! }));
}

/** Markdown export of the given recordings, grouped by day. */
function buildExportMarkdown(recordings: Recording[]): string {
  const groups = groupRecordingsByDay(recordings);
  const lines: string[] = [`# AirNote history`, ``, `${recordings.length} transcript${recordings.length !== 1 ? "s" : ""} · exported ${new Date().toLocaleString()}`, ``];
  for (const g of groups) {
    lines.push(`## ${g.label}`, ``);
    for (const r of g.items) {
      const time = new Date(r.timestamp_ms).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
      const meta = [formatSourceApp(r.target_app), formatModel(r.model_used), time, `${r.word_count} words`, formatDuration(r.recording_seconds)].filter(Boolean).join(" · ");
      lines.push(`**${time}** — ${meta}`, ``, (r.polished || r.transcript || "—").trim(), ``);
      const orig = originalText(r);
      if (orig && orig !== (r.polished ?? "").trim()) lines.push(`> original: ${orig}`, ``);
    }
  }
  return lines.join("\n");
}

// ── Toasts ────────────────────────────────────────────────────────────────────

type ToastKind = "success" | "error" | "info";
interface Toast {
  id: number;
  kind: ToastKind;
  title: string;
  sub?: string;
  action?: { label: string; onClick: () => void };
  duration: number; // ms; 0 = sticky
}

let _toastSeq = 1;

function useToasts() {
  const [toasts, setToasts] = useState<Toast[]>([]);
  const timers = useRef(new Map<number, ReturnType<typeof setTimeout>>());

  const dismiss = useCallback((id: number) => {
    setToasts((prev) => prev.filter((t) => t.id !== id));
    const timer = timers.current.get(id);
    if (timer) { clearTimeout(timer); timers.current.delete(id); }
  }, []);

  const push = useCallback((t: Omit<Toast, "id">): number => {
    const id = _toastSeq++;
    setToasts((prev) => [...prev.slice(-3), { ...t, id }]); // cap the stack at 4
    if (t.duration > 0) {
      timers.current.set(id, setTimeout(() => dismiss(id), t.duration));
    }
    return id;
  }, [dismiss]);

  useEffect(() => () => { timers.current.forEach(clearTimeout); }, []);
  return { toasts, push, dismiss };
}

const TOAST_ICON: Record<ToastKind, React.ReactNode> = {
  success: <Check size={13} strokeWidth={2.5} />,
  error: <AlertTriangle size={13} strokeWidth={2.4} />,
  info: <Trash2 size={13} strokeWidth={2.2} />,
};
const TOAST_TINT: Record<ToastKind, { bg: string; fg: string }> = {
  success: { bg: "hsl(150 60% 50% / 0.16)", fg: "hsl(150 60% 62%)" },
  error: { bg: "hsl(2 70% 60% / 0.16)", fg: "hsl(2 78% 66%)" },
  info: { bg: "hsl(var(--primary) / 0.16)", fg: "hsl(var(--primary))" },
};

function Toaster({ toasts, onDismiss }: { toasts: Toast[]; onDismiss: (id: number) => void }) {
  if (toasts.length === 0) return null;
  return (
    <div className="fixed bottom-5 left-1/2 -translate-x-1/2 z-50 flex flex-col items-center gap-2 pointer-events-none">
      {toasts.map((t) => {
        const tint = TOAST_TINT[t.kind];
        return (
          <div
            key={t.id}
            className="pointer-events-auto flex items-center gap-3 px-4 py-2.5 rounded-2xl max-w-md w-max"
            style={{
              background: "hsl(var(--surface-3))",
              border: "1px solid hsl(var(--border))",
              boxShadow: "0 8px 32px hsl(0 0% 0% / 0.28)",
              animation: "fadeIn 0.18s ease-out",
            }}
          >
            <span className="w-7 h-7 rounded-full flex items-center justify-center flex-shrink-0" style={{ background: tint.bg, color: tint.fg }}>
              {TOAST_ICON[t.kind]}
            </span>
            <div className="flex-1 min-w-0">
              <p className="text-[12px] font-semibold text-foreground leading-tight">{t.title}</p>
              {t.sub && <p className="text-[11px] text-muted-foreground leading-tight mt-0.5 truncate" title={t.sub}>{t.sub}</p>}
            </div>
            {t.action && (
              <button
                onClick={() => { t.action!.onClick(); onDismiss(t.id); }}
                className="flex items-center gap-1 px-2.5 py-1 rounded-lg text-[11px] font-semibold transition-colors flex-shrink-0"
                style={{ color: "hsl(var(--primary))", background: "hsl(var(--primary) / 0.12)" }}
              >
                <Undo2 size={11} /> {t.action.label}
              </button>
            )}
            <button onClick={() => onDismiss(t.id)} title="Dismiss" className="text-muted-foreground hover:text-foreground transition-colors flex-shrink-0">
              <X size={13} />
            </button>
          </div>
        );
      })}
    </div>
  );
}

// ── Filter dropdown ───────────────────────────────────────────────────────────

function FilterDropdown({ label, value, options, onChange }: {
  label: string; value: string; options: { value: string; label: string }[]; onChange: (v: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const h = (e: MouseEvent) => { if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false); };
    document.addEventListener("mousedown", h);
    return () => document.removeEventListener("mousedown", h);
  }, [open]);
  const current = options.find((o) => o.value === value)?.label ?? label;
  return (
    <div ref={ref} className="relative">
      <button
        onClick={() => setOpen((o) => !o)}
        className="flex items-center gap-1.5 px-3 h-8 rounded-lg text-[12px] font-medium transition-colors"
        style={{ background: "hsl(var(--surface-4))", color: "hsl(var(--foreground))", boxShadow: "inset 0 0 0 1px hsl(var(--border))" }}
      >
        {current}
        <ChevronDown size={13} className="text-muted-foreground" />
      </button>
      {open && (
        <div
          className="absolute left-0 top-9 z-40 rounded-xl py-1.5 px-1.5 min-w-[160px] max-h-[260px] overflow-auto"
          style={{ background: "hsl(var(--surface-1))", border: "1px solid hsl(var(--surface-3))", boxShadow: "0 8px 32px rgba(0,0,0,0.4)" }}
        >
          {options.map((o) => (
            <button
              key={o.value}
              onClick={() => { onChange(o.value); setOpen(false); }}
              className="w-full flex items-center justify-between gap-2 px-2.5 py-1.5 text-left text-[12.5px] rounded-lg transition-colors"
              style={{ color: o.value === value ? "hsl(var(--foreground))" : "hsl(var(--muted-foreground))" }}
              onMouseEnter={(e) => { e.currentTarget.style.background = "hsl(var(--surface-4))"; }}
              onMouseLeave={(e) => { e.currentTarget.style.background = "transparent"; }}
            >
              {o.label}
              {o.value === value && <Check size={12} style={{ color: "hsl(var(--primary))" }} />}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

// ── Shared single-player ──────────────────────────────────────────────────────

let _activeAudio: HTMLAudioElement | null = null;
let _activeBlobUrl: string | null = null;
function stopSharedAudio() {
  _activeAudio?.pause();
  _activeAudio = null;
  if (_activeBlobUrl) { URL.revokeObjectURL(_activeBlobUrl); _activeBlobUrl = null; }
}

// ── Row context menu ──────────────────────────────────────────────────────────

interface MenuProps {
  recording: Recording;
  playingId: string | null;
  hasAudio: boolean;
  onPlay: () => void;
  onCopy: () => void;
  onCopyTranscript: () => void;
  onDownload: () => void;
  onDelete: () => void;
  onClose: () => void;
  anchorRef: React.RefObject<HTMLButtonElement | null>;
}
function RowMenu({ recording, playingId, hasAudio, onPlay, onCopy, onCopyTranscript, onDownload, onDelete, onClose, anchorRef }: MenuProps) {
  const menuRef = useRef<HTMLDivElement>(null);
  const isPlaying = playingId === recording.id;
  useEffect(() => {
    function handler(e: MouseEvent) {
      if (menuRef.current && !menuRef.current.contains(e.target as Node) && anchorRef.current && !anchorRef.current.contains(e.target as Node)) onClose();
    }
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, [onClose, anchorRef]);

  const item = (icon: React.ReactNode, label: string, action: () => void, danger = false, disabled = false) => (
    <button
      onClick={() => { if (!disabled) { action(); onClose(); } }}
      disabled={disabled}
      className="w-full flex items-center gap-2.5 px-3 py-2 text-left text-[13px] rounded-lg transition-colors disabled:opacity-40"
      style={{ color: danger ? "hsl(0 75% 62%)" : disabled ? "hsl(var(--muted-foreground))" : "hsl(var(--foreground))" }}
      onMouseEnter={(e) => { if (!disabled) e.currentTarget.style.background = "hsl(var(--surface-4))"; }}
      onMouseLeave={(e) => { e.currentTarget.style.background = "transparent"; }}
    >
      {icon}{label}
    </button>
  );
  return (
    <div ref={menuRef} className="absolute right-0 top-8 z-50 rounded-xl shadow-xl border py-1.5 px-1.5 min-w-[180px]"
      style={{ background: "hsl(var(--surface-1))", borderColor: "hsl(var(--surface-3))", boxShadow: "0 8px 32px rgba(0,0,0,0.4)" }}>
      {item(isPlaying ? <Pause size={13} /> : <Play size={13} />, isPlaying ? "Pause" : "Play recording", onPlay, false, !hasAudio)}
      {item(<Copy size={13} />, "Copy polished text", onCopy)}
      {item(<Copy size={13} />, "Copy original", onCopyTranscript)}
      {item(<Download size={13} />, "Download audio", onDownload, false, !hasAudio)}
      <div className="my-1 mx-1 border-t" style={{ borderColor: "hsl(var(--surface-3))" }} />
      {item(<Trash2 size={13} />, "Delete", onDelete, true)}
    </div>
  );
}

// ── History row ───────────────────────────────────────────────────────────────

const TRUNCATE_WORD_LIMIT = 60;

interface RowProps {
  recording: Recording;
  playingId: string | null;
  onPlay: (r: Recording) => void;
  onDelete: (r: Recording) => void;
  onCopyToast: (kind: ToastKind, title: string) => void;
  onDownloadSuccess?: (path: string) => void;
  onDownloadError: (msg: string) => void;
}

function HistoryRow({ recording, playingId, onPlay, onDelete, onCopyToast, onDownloadSuccess, onDownloadError }: RowProps) {
  const [menuOpen, setMenuOpen] = useState(false);
  const [copied, setCopied] = useState<"polished" | "transcript" | false>(false);
  const [expanded, setExpanded] = useState(false);
  const [showOriginal, setShowOriginal] = useState(false);
  const [downloading, setDownloading] = useState(false);
  const [hasAudio, setHasAudio] = useState(Boolean(recording.audio_id));
  const [appIcon, setAppIcon] = useState<string | null>(() =>
    recording.target_app ? appIconCache.get(recording.target_app) ?? null : null,
  );
  const btnRef = useRef<HTMLButtonElement>(null);

  // Resolve the icon of the app this dictation was pasted into. Cached + deduped
  // module-side, so this is a no-op for apps already seen.
  useEffect(() => {
    const key = recording.target_app;
    if (!key || !key.trim()) { setAppIcon(null); return; }
    let alive = true;
    void resolveAppIcon(key).then((url) => { if (alive) setAppIcon(url); });
    return () => { alive = false; };
  }, [recording.target_app]);

  useEffect(() => {
    let alive = true;
    if (recording.audio_id) { setHasAudio(true); return; }
    void getRecordingAudioBytes(recording.id).then((bytes) => {
      if (alive && bytes && bytes.length > 0) setHasAudio(true);
    });
    return () => { alive = false; };
  }, [recording.id, recording.audio_id]);

  async function handleDownload() {
    if (!hasAudio || downloading) return;
    setDownloading(true);
    try {
      const savedPath = await downloadRecordingAudio(recording);
      if (savedPath) onDownloadSuccess?.(savedPath);
    } catch (e) {
      onDownloadError(friendlyError(e, "Couldn’t save the audio."));
    } finally {
      setDownloading(false);
    }
  }

  const time = new Date(recording.timestamp_ms).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  const isPlaying = playingId === recording.id;

  const fullText = (recording.polished ?? "").trim();
  const wordCount = recording.word_count ?? fullText.split(/\s+/).filter(Boolean).length;
  const isLong = wordCount > TRUNCATE_WORD_LIMIT;
  const displayText = useMemo(() => {
    if (!isLong || expanded) return fullText;
    const parts = fullText.split(/(\s+)/);
    let kept = 0, cut = parts.length;
    for (let i = 0; i < parts.length; i++) {
      if (parts[i].trim()) { kept++; if (kept >= TRUNCATE_WORD_LIMIT) { cut = i + 1; break; } }
    }
    return parts.slice(0, cut).join("").trimEnd();
  }, [fullText, isLong, expanded]);

  const orig = originalText(recording);
  const hasOriginal = orig.length > 0 && orig !== fullText;
  const source = formatSourceApp(recording.target_app);
  const model = formatModel(recording.model_used);
  const duration = formatDuration(recording.recording_seconds);

  function handleCopy() {
    navigator.clipboard.writeText(recording.polished ?? recording.transcript ?? "");
    setCopied("polished"); setTimeout(() => setCopied(false), 1600);
    onCopyToast("success", "Copied to clipboard");
  }
  function handleCopyTranscript() {
    navigator.clipboard.writeText(orig || recording.transcript || "");
    setCopied("transcript"); setTimeout(() => setCopied(false), 1600);
    onCopyToast("success", "Original copied");
  }

  return (
    <div
      className="hist-card group relative flex gap-3 rounded-xl px-4 py-3.5 transition-colors"
      style={{ boxShadow: "inset 0 0 0 1px hsl(var(--border))", background: "hsl(var(--surface-1))" }}
      onMouseEnter={(e) => { e.currentTarget.style.background = "hsl(var(--surface-2))"; }}
      onMouseLeave={(e) => { e.currentTarget.style.background = "hsl(var(--surface-1))"; }}
    >
      {/* Source-app anchor — real app icon where the dictation was pasted, with
          a generic monitor fallback when we couldn't resolve one. */}
      <div
        className="w-8 h-8 flex-shrink-0 rounded-lg flex items-center justify-center mt-0.5 overflow-hidden"
        style={{ background: "hsl(var(--surface-4))", color: "hsl(var(--muted-foreground))" }}
        title={source}
      >
        {appIcon ? (
          <img
            src={appIcon}
            alt={source}
            className="w-full h-full object-contain"
            draggable={false}
          />
        ) : (
          <Monitor size={15} />
        )}
      </div>

      {/* Body */}
      <div className="flex-1 min-w-0">
        <p className="text-[14.5px] text-foreground leading-relaxed">
          {fullText ? (
            <>
              {displayText}
              {isLong && (
                <button
                  onClick={() => setExpanded((v) => !v)}
                  className="ml-1 text-[12.5px] font-semibold transition-colors align-baseline"
                  style={{ color: expanded ? "hsl(var(--muted-foreground))" : "hsl(var(--primary))" }}
                >
                  {expanded ? "Show less" : "… more"}
                </button>
              )}
            </>
          ) : (
            <span className="italic text-muted-foreground">No text — nothing was typed for this take.</span>
          )}
        </p>

        {/* Original (progressive disclosure) */}
        {showOriginal && hasOriginal && (
          <div className="mt-2 px-3 py-2 rounded-lg" style={{ background: "hsl(var(--surface-4))" }}>
            <div className="flex items-center justify-between mb-1">
              <span className="text-[10px] font-medium uppercase tracking-wide text-muted-foreground">
                Original {recording.enriched_transcript ? "(with confidence)" : "transcript"}
              </span>
              <button onClick={handleCopyTranscript} className="text-[10px] flex items-center gap-1 transition-colors"
                style={{ color: copied === "transcript" ? "hsl(var(--chip-lime-fg))" : "hsl(var(--muted-foreground))" }}>
                {copied === "transcript" ? <Check size={10} /> : <Copy size={10} />}
                {copied === "transcript" ? "Copied" : "Copy"}
              </button>
            </div>
            <p className="text-[12.5px] text-muted-foreground leading-relaxed font-mono">{orig}</p>
          </div>
        )}

        {/* Meta line */}
        <div className="flex items-center gap-1.5 mt-2 text-[11px] text-muted-foreground flex-wrap">
          <span>{source}</span>
          {model && <><span className="opacity-40">·</span><span>{model}</span></>}
          <span className="opacity-40">·</span><span className="tabular-nums">{time}</span>
          <span className="opacity-40">·</span><span className="tabular-nums">{wordCount} words</span>
          {duration && <><span className="opacity-40">·</span><span className="tabular-nums">{duration}</span></>}
          {hasOriginal && (
            <>
              <span className="opacity-40">·</span>
              <button onClick={() => setShowOriginal((v) => !v)} className="font-medium transition-colors" style={{ color: "hsl(var(--chip-lime-fg))" }}>
                {showOriginal ? "Hide original" : "Show original"}
              </button>
            </>
          )}
          {isPlaying && (
            <>
              <span className="opacity-40">·</span>
              <span className="flex items-center gap-1" style={{ color: "hsl(var(--chip-lime-fg))" }}>
                <span className="inline-block w-1.5 h-1.5 rounded-full bg-current animate-pulse" />Playing
              </span>
            </>
          )}
        </div>
      </div>

      {/* Hover actions */}
      <div className="hist-actions flex-shrink-0 flex items-start gap-0.5 opacity-0 group-hover:opacity-100 transition-opacity">
        <button onClick={handleCopy} title="Copy polished text"
          className="w-7 h-7 rounded-lg flex items-center justify-center transition-colors"
          style={{ color: copied === "polished" ? "hsl(var(--chip-lime-fg))" : "hsl(var(--muted-foreground))" }}
          onMouseEnter={(e) => { e.currentTarget.style.background = "hsl(var(--surface-4))"; }}
          onMouseLeave={(e) => { e.currentTarget.style.background = "transparent"; }}>
          {copied === "polished" ? <Check size={13} /> : <Copy size={13} />}
        </button>
        {hasAudio && (
          <button type="button" onClick={(e) => { e.stopPropagation(); onPlay(recording); }} title={isPlaying ? "Pause" : "Play"}
            className="w-7 h-7 rounded-lg flex items-center justify-center transition-colors"
            style={{ color: isPlaying ? "hsl(var(--primary))" : "hsl(var(--muted-foreground))" }}
            onMouseEnter={(e) => { e.currentTarget.style.background = "hsl(var(--surface-4))"; }}
            onMouseLeave={(e) => { e.currentTarget.style.background = "transparent"; }}>
            {isPlaying ? <Pause size={13} /> : <Play size={13} />}
          </button>
        )}
        <div className="relative">
          <button ref={btnRef} onClick={() => setMenuOpen((o) => !o)} title="More options"
            className="w-7 h-7 rounded-lg flex items-center justify-center transition-colors"
            style={{ color: menuOpen ? "hsl(var(--foreground))" : "hsl(var(--muted-foreground))", background: menuOpen ? "hsl(var(--surface-4))" : "transparent" }}
            onMouseEnter={(e) => { e.currentTarget.style.background = "hsl(var(--surface-4))"; }}
            onMouseLeave={(e) => { if (!menuOpen) e.currentTarget.style.background = "transparent"; }}>
            <MoreHorizontal size={14} />
          </button>
          {menuOpen && (
            <RowMenu recording={recording} playingId={playingId} hasAudio={hasAudio}
              onPlay={() => onPlay(recording)} onCopy={handleCopy} onCopyTranscript={handleCopyTranscript}
              onDownload={handleDownload} onDelete={() => onDelete(recording)} onClose={() => setMenuOpen(false)} anchorRef={btnRef} />
          )}
        </div>
      </div>
    </div>
  );
}

// ── Skeleton ──────────────────────────────────────────────────────────────────

function Skeleton() {
  return (
    <div className="space-y-3">
      {[0, 1, 2, 3].map((i) => (
        <div key={i} className="flex gap-3 rounded-xl px-4 py-3.5" style={{ boxShadow: "inset 0 0 0 1px hsl(var(--border))" }}>
          <div className="w-8 h-8 rounded-lg animate-pulse" style={{ background: "hsl(var(--surface-4))" }} />
          <div className="flex-1 space-y-2 pt-1">
            <div className="h-3 rounded animate-pulse" style={{ background: "hsl(var(--surface-4))", width: `${70 - i * 8}%` }} />
            <div className="h-2.5 rounded animate-pulse" style={{ background: "hsl(var(--surface-4))", width: "40%" }} />
          </div>
        </div>
      ))}
    </div>
  );
}

// ── Main view ─────────────────────────────────────────────────────────────────

// One page of history. We load the most-recent page and let the user pull older
// pages on demand ("Load older"), instead of holding the whole table in memory.
// ~50 ≈ a day of normal use; older days are one click away.
const PAGE_SIZE = 50;

export function HistoryView({ onDownloadSuccess, refreshKey }: { onDownloadSuccess?: (path: string) => void; refreshKey?: number }) {
  const [recordings, setRecordings] = useState<Recording[]>([]);
  const [loading, setLoading] = useState(true);
  const [query, setQuery] = useState("");
  const [sourceFilter, setSourceFilter] = useState("all");
  const [timeFilter, setTimeFilter] = useState("all");
  const [refreshing, setRefreshing] = useState(false);
  const [playingId, setPlayingId] = useState<string | null>(null);
  const [confirmClear, setConfirmClear] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [hasMore, setHasMore] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  const { toasts, push, dismiss } = useToasts();
  const pendingDeletes = useRef(new Map<string, { timer: ReturnType<typeof setTimeout>; commit: () => void }>());

  const loadHistory = useCallback(async (soft = false) => {
    if (soft) setRefreshing(true);
    try {
      const recs = await listHistory(PAGE_SIZE);
      setRecordings(recs);
      setHasMore(recs.length >= PAGE_SIZE);
    } finally {
      setLoading(false);
      setRefreshing(false);
    }
  }, []);

  // Optimistic "load older": fetch the next page using the oldest loaded row as
  // the cursor and append it. Recordings come back newest-first, so the tail is
  // the oldest we currently hold.
  const loadMore = useCallback(async () => {
    setLoadingMore(true);
    try {
      const oldest = recordings[recordings.length - 1]?.timestamp_ms;
      const older = await listHistory(PAGE_SIZE, oldest);
      if (older.length > 0) {
        setRecordings((prev) => {
          const seen = new Set(prev.map((r) => r.id));
          return [...prev, ...older.filter((r) => !seen.has(r.id))];
        });
      }
      setHasMore(older.length >= PAGE_SIZE);
    } finally {
      setLoadingMore(false);
    }
  }, [recordings]);

  useEffect(() => { void loadHistory(); }, [loadHistory, refreshKey]);
  useEffect(() => () => {
    stopSharedAudio();
    // Flush pending deletes so a delete made just before leaving still persists
    // (otherwise the item would silently reappear on the next load).
    pendingDeletes.current.forEach(({ timer, commit }) => { clearTimeout(timer); commit(); });
    pendingDeletes.current.clear();
  }, []);

  // Distinct source apps for the filter.
  const sourceOptions = useMemo(() => {
    const set = new Map<string, string>();
    for (const r of recordings) {
      const label = formatSourceApp(r.target_app);
      set.set(label, label);
    }
    return [{ value: "all", label: "All sources" }, ...[...set.keys()].sort().map((s) => ({ value: s, label: s }))];
  }, [recordings]);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    const now = Date.now();
    const timeCut = timeFilter === "today" ? new Date().setHours(0, 0, 0, 0)
      : timeFilter === "7d" ? now - 7 * 86_400_000
      : timeFilter === "30d" ? now - 30 * 86_400_000
      : 0;
    return recordings.filter((r) => {
      if (timeCut && r.timestamp_ms < timeCut) return false;
      if (sourceFilter !== "all" && formatSourceApp(r.target_app) !== sourceFilter) return false;
      if (q) {
        const hay = `${r.polished ?? ""} ${r.transcript ?? ""} ${r.final_text ?? ""}`.toLowerCase();
        if (!hay.includes(q)) return false;
      }
      return true;
    });
  }, [recordings, query, sourceFilter, timeFilter]);

  const insertSorted = (list: Recording[], rec: Recording) =>
    [...list, rec].sort((a, b) => b.timestamp_ms - a.timestamp_ms);

  // Delete = optimistic removal + 5s Undo window before the backend call fires.
  function handleDelete(rec: Recording) {
    if (playingId === rec.id) { stopSharedAudio(); setPlayingId(null); }
    setRecordings((prev) => prev.filter((r) => r.id !== rec.id));
    const commit = () => {
      pendingDeletes.current.delete(rec.id);
      deleteRecording(rec.id).catch(() => {
        push({ kind: "error", title: "Couldn’t delete", sub: "It’s back in your history.", duration: 4000 });
        setRecordings((prev) => insertSorted(prev, rec));
      });
    };
    const timer = setTimeout(commit, 5000);
    pendingDeletes.current.set(rec.id, { timer, commit });
    push({
      kind: "info", title: "Recording deleted", duration: 5000,
      action: {
        label: "Undo",
        onClick: () => {
          const e = pendingDeletes.current.get(rec.id);
          if (e) { clearTimeout(e.timer); pendingDeletes.current.delete(rec.id); }
          setRecordings((prev) => insertSorted(prev, rec));
        },
      },
    });
  }

  // Clear all = optimistic wipe + one 5s Undo before the batch delete fires.
  function handleClearAll() {
    setConfirmClear(false);
    const snapshot = recordings;
    if (snapshot.length === 0) return;
    if (playingId) { stopSharedAudio(); setPlayingId(null); }
    setRecordings([]);
    const commit = () => {
      pendingDeletes.current.delete("__all__");
      void Promise.allSettled(snapshot.map((r) => deleteRecording(r.id))).then((res) => {
        const failed = res.filter((r) => r.status === "rejected").length;
        if (failed > 0) {
          push({ kind: "error", title: `Couldn’t delete ${failed} recording${failed !== 1 ? "s" : ""}`, sub: "Refreshing…", duration: 4000 });
          void loadHistory(true);
        }
      });
    };
    const timer = setTimeout(commit, 5000);
    pendingDeletes.current.set("__all__", { timer, commit });
    push({
      kind: "info", title: `Cleared ${snapshot.length} recording${snapshot.length !== 1 ? "s" : ""}`, duration: 5000,
      action: {
        label: "Undo",
        onClick: () => {
          const e = pendingDeletes.current.get("__all__");
          if (e) { clearTimeout(e.timer); pendingDeletes.current.delete("__all__"); }
          setRecordings(snapshot);
        },
      },
    });
  }

  async function handleExport() {
    if (exporting) return;
    const set = filtered;
    if (set.length === 0) { push({ kind: "info", title: "Nothing to export", duration: 2500 }); return; }
    setExporting(true);
    try {
      const d = new Date();
      const stamp = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
      const path = await exportHistory(buildExportMarkdown(set), `airnote-history-${stamp}.md`);
      if (path) {
        push({ kind: "success", title: `Exported ${set.length} transcript${set.length !== 1 ? "s" : ""}`, sub: path, duration: 6000,
          action: { label: "Reveal", onClick: () => void revealDownloadedFile(path) } });
      }
    } catch (e) {
      push({ kind: "error", title: "Export failed", sub: friendlyError(e), duration: 5000 });
    } finally {
      setExporting(false);
    }
  }

  async function handlePlay(rec: Recording) {
    if (playingId === rec.id) { stopSharedAudio(); setPlayingId(null); return; }
    try {
      const bytes = await getRecordingAudioBytes(rec.id);
      if (!bytes || bytes.length === 0) { push({ kind: "error", title: "No audio for this take", duration: 2500 }); return; }
      stopSharedAudio();
      const blob = new Blob([bytes as BlobPart], { type: "audio/wav" });
      const url = URL.createObjectURL(blob);
      const audio = new Audio(url);
      _activeAudio = audio; _activeBlobUrl = url;
      audio.onended = () => { setPlayingId(null); stopSharedAudio(); };
      audio.onerror = () => { setPlayingId(null); stopSharedAudio(); push({ kind: "error", title: "Couldn’t play audio", duration: 3000 }); };
      await audio.play();
      setPlayingId(rec.id);
    } catch {
      setPlayingId(null); stopSharedAudio();
      push({ kind: "error", title: "Couldn’t play audio", duration: 3000 });
    }
  }

  const timeline = groupRecordingsByDay(filtered);
  const hasFilters = query.trim().length > 0 || sourceFilter !== "all" || timeFilter !== "all";

  return (
    <ScrollArea className="h-full">
      <div className="p-7 pb-16 max-w-3xl mx-auto">
        {/* Header */}
        <div className="flex items-start justify-between mb-5 gap-4">
          <div className="min-w-0">
            <h1 className="text-[28px] font-bold tracking-tight text-foreground leading-tight">History</h1>
            <p className="text-[13px] text-muted-foreground mt-1 tabular-nums">
              {recordings.length} transcript{recordings.length !== 1 ? "s" : ""} · stored on this device
            </p>
          </div>
          {recordings.length > 0 && (
            <div className="flex items-center gap-2 flex-shrink-0 mt-1">
              <button onClick={() => void handleExport()} disabled={exporting}
                className="flex items-center gap-1.5 h-8 px-3 rounded-lg text-[12px] font-medium transition-colors disabled:opacity-50"
                style={{ background: "hsl(var(--surface-4))", color: "hsl(var(--foreground))", boxShadow: "inset 0 0 0 1px hsl(var(--border))" }}>
                <FileDown size={13} /> {exporting ? "Exporting…" : "Export"}
              </button>
              {confirmClear ? (
                <div className="flex items-center gap-1.5">
                  <button onClick={handleClearAll} className="h-8 px-3 rounded-lg text-[12px] font-semibold text-white" style={{ background: "hsl(0 72% 51%)" }}>
                    Clear {recordings.length}
                  </button>
                  <button onClick={() => setConfirmClear(false)} className="h-8 px-3 rounded-lg text-[12px] text-muted-foreground">Cancel</button>
                </div>
              ) : (
                <button onClick={() => setConfirmClear(true)}
                  className="flex items-center gap-1.5 h-8 px-3 rounded-lg text-[12px] font-medium transition-colors"
                  style={{ color: "hsl(0 75% 62%)", background: "hsl(0 72% 51% / 0.1)" }}>
                  <Trash2 size={13} /> Clear all
                </button>
              )}
              <button onClick={() => void loadHistory(true)} disabled={refreshing} title="Refresh"
                className="w-8 h-8 rounded-lg flex items-center justify-center transition-colors"
                style={{ background: "hsl(var(--surface-4))", color: "hsl(var(--muted-foreground))" }}>
                <RefreshCw size={13} className={refreshing ? "animate-spin" : ""} />
              </button>
            </div>
          )}
        </div>

        {/* Loading */}
        {loading ? (
          <Skeleton />
        ) : recordings.length === 0 ? (
          <div className="h-[50vh] flex items-center justify-center">
            <div className="text-center px-8">
              <div className="w-12 h-12 rounded-full flex items-center justify-center mx-auto mb-4" style={{ background: "hsl(var(--primary) / 0.15)" }}>
                <Clock size={20} style={{ color: "hsl(var(--chip-lime-fg))" }} />
              </div>
              <p className="text-[14px] font-semibold text-foreground mb-1">No history yet</p>
              <p className="text-[12px] text-muted-foreground max-w-xs leading-relaxed">
                Hold your hotkey and speak — your dictations will show up here.
              </p>
            </div>
          </div>
        ) : (
          <>
            {/* Search + filters */}
            <div className="flex items-center gap-2 mb-4 flex-wrap">
              <div className="flex items-center gap-2 px-3 h-9 rounded-xl flex-1 min-w-[200px]"
                style={{ background: "hsl(var(--surface-4))", boxShadow: "inset 0 0 0 1px hsl(var(--border))" }}>
                <Search size={13} className="text-muted-foreground flex-shrink-0" />
                <input type="text" value={query} onChange={(e) => setQuery(e.target.value)} placeholder="Search transcripts…"
                  className="flex-1 bg-transparent outline-none text-[12.5px] text-foreground placeholder:text-muted-foreground/70" />
                {query && (
                  <button onClick={() => setQuery("")} className="text-muted-foreground hover:text-foreground transition-colors" title="Clear search">
                    <X size={12} />
                  </button>
                )}
              </div>
              <FilterDropdown label="All sources" value={sourceFilter} options={sourceOptions} onChange={setSourceFilter} />
              <FilterDropdown label="All time" value={timeFilter} onChange={setTimeFilter}
                options={[{ value: "all", label: "All time" }, { value: "today", label: "Today" }, { value: "7d", label: "Last 7 days" }, { value: "30d", label: "Last 30 days" }]} />
            </div>

            {/* No results */}
            {filtered.length === 0 ? (
              <div className="text-center py-16">
                <p className="text-[13px] text-foreground font-medium mb-1">No matches</p>
                <p className="text-[12px] text-muted-foreground">
                  {hasFilters ? "Try a different search or filter." : "Nothing here yet."}
                </p>
                {hasFilters && (
                  <button onClick={() => { setQuery(""); setSourceFilter("all"); setTimeFilter("all"); }}
                    className="mt-3 text-[12px] font-semibold" style={{ color: "hsl(var(--primary))" }}>
                    Clear filters
                  </button>
                )}
              </div>
            ) : (
              <>
                <div className="space-y-7">
                  {timeline.map((group) => (
                    <div key={group.label}>
                      <div className="flex items-center gap-2 mb-2.5 px-1">
                        <span className="text-[12px] font-semibold text-foreground">{group.label}</span>
                        <span className="text-[11px] text-muted-foreground tabular-nums">· {group.items.length}</span>
                      </div>
                      <div className="space-y-2">
                        {group.items.map((rec) => (
                          <HistoryRow key={rec.id} recording={rec} playingId={playingId}
                            onPlay={handlePlay} onDelete={handleDelete}
                            onCopyToast={(kind, title) => push({ kind, title, duration: 2000 })}
                            onDownloadSuccess={onDownloadSuccess}
                            onDownloadError={(msg) => push({ kind: "error", title: "Download failed", sub: msg, duration: 4000 })} />
                        ))}
                      </div>
                    </div>
                  ))}
                </div>
                {hasMore && (
                  <div className="flex justify-center mt-6">
                    <button
                      onClick={() => void loadMore()}
                      disabled={loadingMore}
                      className="text-[12px] font-semibold px-4 py-2 rounded-lg transition-colors disabled:opacity-50"
                      style={{ color: "hsl(var(--primary))", background: "hsl(var(--surface-2))" }}
                    >
                      {loadingMore ? "Loading…" : "Load older"}
                    </button>
                  </div>
                )}
              </>
            )}
          </>
        )}
      </div>

      <Toaster toasts={toasts} onDismiss={dismiss} />
    </ScrollArea>
  );
}
