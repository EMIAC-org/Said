import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Clock, Copy, Play, Pause, Trash2, MoreHorizontal, Check, Search, X, Download, RefreshCw } from "lucide-react";
import { ScrollArea } from "@/components/ui/scroll-area";
import type { Recording } from "@/types";
import {
  deleteRecording,
  listHistory,
  downloadRecordingAudio as saveRecordingAudio,
  getRecordingAudioBytes,
} from "@/lib/invoke";

// ── Download helper ───────────────────────────────────────────────────────────

/** Build a friendly filename: "said-2026-05-03-1430-12-words.wav". */
function audioFilename(recording: Recording): string {
  const d     = new Date(recording.timestamp_ms);
  const pad   = (n: number) => String(n).padStart(2, "0");
  const stamp = `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}-${pad(d.getHours())}${pad(d.getMinutes())}`;
  return `said-${stamp}-${recording.word_count}-words.wav`;
}

/** Ask Tauri to save the WAV. Native app prompts for the destination path. */
async function downloadRecordingAudio(recording: Recording): Promise<string | null> {
  return await saveRecordingAudio(recording.id, audioFilename(recording));
}

/** Group full Recording rows by calendar day — preserves ids (unlike groupHistory). */
function groupRecordingsByDay(recordings: Recording[]): { label: string; items: Recording[] }[] {
  if (recordings.length === 0) return [];

  const now = Date.now();
  const startOfToday = new Date(now);
  startOfToday.setHours(0, 0, 0, 0);
  const todayMs = startOfToday.getTime();
  const yesterdayMs = todayMs - 86_400_000;

  const buckets = new Map<string, Recording[]>();
  const order: string[] = [];

  for (const rec of recordings) {
    const d = new Date(rec.timestamp_ms);
    const startOfItemDay = new Date(rec.timestamp_ms);
    startOfItemDay.setHours(0, 0, 0, 0);
    const itemDayMs = startOfItemDay.getTime();

    let label: string;
    if (itemDayMs >= todayMs) {
      label = "TODAY";
    } else if (itemDayMs >= yesterdayMs) {
      label = "YESTERDAY";
    } else {
      label = d
        .toLocaleDateString("en-US", {
          month: "long",
          day: "numeric",
          year: "numeric",
        })
        .toUpperCase();
    }

    if (!buckets.has(label)) {
      buckets.set(label, []);
      order.push(label);
    }
    buckets.get(label)!.push(rec);
  }

  return order.map((label) => ({ label, items: buckets.get(label)! }));
}

/** One shared player — same pattern as the dashboard history rows. */
let _activeAudio: HTMLAudioElement | null = null;
let _activeBlobUrl: string | null = null;

function stopSharedAudio() {
  _activeAudio?.pause();
  _activeAudio = null;
  if (_activeBlobUrl) {
    URL.revokeObjectURL(_activeBlobUrl);
    _activeBlobUrl = null;
  }
}

// ── Context menu ──────────────────────────────────────────────────────────────

interface MenuProps {
  recording:   Recording;
  playingId:   string | null;
  hasAudio:    boolean;
  onPlay:      () => void;
  onCopy:      () => void;
  onCopyTranscript: () => void;
  onDownload:  () => void;
  onDelete:    () => void;
  onClose:     () => void;
  anchorRef:   React.RefObject<HTMLButtonElement | null>;
}

function RowMenu({ recording, playingId, hasAudio, onPlay, onCopy, onCopyTranscript, onDownload, onDelete, onClose, anchorRef }: MenuProps) {
  const menuRef  = useRef<HTMLDivElement>(null);
  const isPlaying = playingId === recording.id;

  // Close on outside click
  useEffect(() => {
    function handler(e: MouseEvent) {
      if (
        menuRef.current && !menuRef.current.contains(e.target as Node) &&
        anchorRef.current && !anchorRef.current.contains(e.target as Node)
      ) {
        onClose();
      }
    }
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, [onClose, anchorRef]);

  const item = (
    icon: React.ReactNode,
    label: string,
    action: () => void,
    danger = false,
    disabled = false,
  ) => (
    <button
      onClick={() => { if (!disabled) { action(); onClose(); } }}
      disabled={disabled}
      className="w-full flex items-center gap-2.5 px-3 py-2 text-left text-[13px] rounded-lg transition-colors disabled:opacity-40"
      style={{
        color: danger ? "hsl(0 75% 62%)" : disabled ? "hsl(var(--muted-foreground))" : "hsl(var(--foreground))",
      }}
      onMouseEnter={(e) => {
        if (!disabled) e.currentTarget.style.background = "hsl(var(--surface-4))";
      }}
      onMouseLeave={(e) => { e.currentTarget.style.background = "transparent"; }}
    >
      {icon}
      {label}
    </button>
  );

  return (
    <div
      ref={menuRef}
      className="absolute right-0 top-8 z-50 rounded-xl shadow-xl border py-1.5 px-1.5 min-w-[180px]"
      style={{
        background: "hsl(var(--surface-1))",
        borderColor: "hsl(var(--surface-3))",
        boxShadow: "0 8px 32px rgba(0,0,0,0.4)",
      }}
    >
      {item(
        isPlaying ? <Pause size={13} /> : <Play size={13} />,
        isPlaying ? "Pause" : "Play recording",
        onPlay,
        false,
        !hasAudio,
      )}
      {item(<Copy size={13} />, "Copy polished text", onCopy)}
      {item(<Copy size={13} />, "Copy STT transcript", onCopyTranscript)}
      {item(<Download size={13} />, "Download audio", onDownload, false, !hasAudio)}
      <div className="my-1 mx-1 border-t" style={{ borderColor: "hsl(var(--surface-3))" }} />
      {item(<Trash2 size={13} />, "Delete", onDelete, true)}
    </div>
  );
}

// ── Single history row ────────────────────────────────────────────────────────

interface RowProps {
  recording:   Recording;
  playingId:   string | null;
  onPlay:      (r: Recording) => void;
  onDelete:    (r: Recording) => void;
  onDownloadSuccess?: (path: string) => void;
}

/** Polished history text longer than this many words is collapsed behind a "Read more" toggle. */
const TRUNCATE_WORD_LIMIT = 50;

function HistoryRow({ recording, playingId, onPlay, onDelete, onDownloadSuccess }: RowProps) {
  const [menuOpen,    setMenuOpen]    = useState(false);
  const [copied,      setCopied]      = useState<"polished" | "transcript" | false>(false);
  const [expanded,    setExpanded]    = useState(false);
  const [downloading, setDownloading] = useState(false);
  const [hasAudio,    setHasAudio]    = useState(Boolean(recording.audio_id));
  const btnRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    let alive = true;
    if (recording.audio_id) {
      setHasAudio(true);
      return;
    }
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
    } finally {
      setDownloading(false);
    }
  }

  const time = new Date(recording.timestamp_ms).toLocaleTimeString([], {
    hour: "2-digit", minute: "2-digit",
  });

  const isPlaying = playingId === recording.id;

  // Truncate long polished text — split on whitespace, keep the first N words
  const fullText  = recording.polished ?? "";
  const wordParts = fullText.split(/(\s+)/);   // keep delimiters so re-join preserves spacing
  const wordCount = recording.word_count ?? fullText.trim().split(/\s+/).filter(Boolean).length;
  const isLong    = wordCount > TRUNCATE_WORD_LIMIT;
  // Collect tokens until we've kept TRUNCATE_WORD_LIMIT non-whitespace tokens
  let kept = 0;
  let cutIdx = wordParts.length;
  for (let i = 0; i < wordParts.length; i++) {
    if (wordParts[i].trim().length > 0) {
      kept += 1;
      if (kept >= TRUNCATE_WORD_LIMIT) {
        cutIdx = i + 1;
        break;
      }
    }
  }
  const truncatedText = wordParts.slice(0, cutIdx).join("").trimEnd();
  const displayText   = !isLong || expanded ? fullText : truncatedText;

  function handleCopy() {
    navigator.clipboard.writeText(recording.polished ?? recording.transcript ?? "");
    setCopied("polished");
    setTimeout(() => setCopied(false), 1800);
  }

  function handleCopyTranscript() {
    navigator.clipboard.writeText(recording.enriched_transcript ?? recording.transcript ?? "");
    setCopied("transcript");
    setTimeout(() => setCopied(false), 1800);
  }

  return (
    <div
      className="relative flex gap-4 px-5 py-4 transition-colors group"
      onMouseEnter={(e) => { e.currentTarget.style.background = "hsl(var(--surface-hover))"; }}
      onMouseLeave={(e) => { e.currentTarget.style.background = "transparent"; }}
    >
      {/* Timestamp */}
      <div className="w-20 flex-shrink-0 pt-0.5">
        <div className="flex items-center gap-1 text-[11px] text-muted-foreground tabular-nums">
          <Clock size={10} className="opacity-70" />
          <span>{time}</span>
        </div>
      </div>

      {/* Content */}
      <div className="flex-1 min-w-0">
        <p className="text-[14px] text-foreground leading-relaxed">
          {fullText ? (
            <>
              {displayText}
              {isLong && !expanded && (
                <>
                  <span className="text-muted-foreground">… </span>
                  <button
                    onClick={() => setExpanded(true)}
                    className="text-[12.5px] font-semibold transition-colors"
                    style={{ color: "hsl(var(--primary))" }}
                  >
                    Read more
                  </button>
                </>
              )}
              {isLong && expanded && (
                <>
                  {" "}
                  <button
                    onClick={() => setExpanded(false)}
                    className="text-[12.5px] font-semibold transition-colors"
                    style={{ color: "hsl(var(--muted-foreground))" }}
                  >
                    Show less
                  </button>
                </>
              )}
            </>
          ) : (
            <span className="italic text-muted-foreground">—</span>
          )}
        </p>
        {/* STT Transcript — shown when expanded (long texts) or always for short texts with different transcript */}
        {(expanded || !isLong) && recording.transcript && recording.transcript !== recording.polished && (
          <div className="mt-2.5 px-3 py-2 rounded-lg" style={{ background: "hsl(var(--surface-4))" }}>
            <div className="flex items-center justify-between mb-1">
              <span className="text-[10px] font-medium uppercase tracking-wide text-muted-foreground">
                STT Transcript {recording.enriched_transcript ? "(with confidence)" : ""}
              </span>
              <button
                onClick={handleCopyTranscript}
                className="text-[10px] flex items-center gap-1 transition-colors"
                style={{ color: copied === "transcript" ? "hsl(var(--chip-lime-fg))" : "hsl(var(--muted-foreground))" }}
              >
                {copied === "transcript" ? <Check size={10} /> : <Copy size={10} />}
                {copied === "transcript" ? "Copied" : "Copy"}
              </button>
            </div>
            <p className="text-[12.5px] text-muted-foreground leading-relaxed font-mono">
              {recording.enriched_transcript ?? recording.transcript}
            </p>
          </div>
        )}

        <div className="flex items-center gap-3 mt-2 flex-wrap">
          {recording.word_count != null && (
            <span className="text-[11px] text-muted-foreground tabular-nums">
              {recording.word_count} words
            </span>
          )}
          {isPlaying && (
            <span className="text-[11px] flex items-center gap-1" style={{ color: "hsl(var(--chip-lime-fg))" }}>
              <span className="inline-block w-1.5 h-1.5 rounded-full bg-current animate-pulse" />
              Playing…
            </span>
          )}
        </div>
      </div>

      {/* Action buttons — visible on hover */}
      <div className="flex-shrink-0 flex items-center gap-1 opacity-100">
        {/* Quick copy */}
        <button
          onClick={handleCopy}
          title="Copy polished text"
          className="w-7 h-7 rounded-lg flex items-center justify-center transition-colors"
          style={{ color: copied === "polished" ? "hsl(var(--chip-lime-fg))" : "hsl(var(--muted-foreground))" }}
          onMouseEnter={(e) => { e.currentTarget.style.background = "hsl(var(--surface-4))"; }}
          onMouseLeave={(e) => { e.currentTarget.style.background = "transparent"; }}
        >
          {copied === "polished" ? <Check size={13} /> : <Copy size={13} />}
        </button>

        {/* Quick play — only when audio exists */}
        {hasAudio && (
          <button
            type="button"
            onClick={(e) => { e.stopPropagation(); onPlay(recording); }}
            title={isPlaying ? "Pause" : "Play"}
            className="w-7 h-7 rounded-lg flex items-center justify-center transition-colors"
            style={{ color: isPlaying ? "hsl(var(--primary))" : "hsl(var(--muted-foreground))" }}
            onMouseEnter={(e) => { e.currentTarget.style.background = "hsl(var(--surface-4))"; }}
            onMouseLeave={(e) => { e.currentTarget.style.background = "transparent"; }}
          >
            {isPlaying ? <Pause size={13} /> : <Play size={13} />}
          </button>
        )}

        {/* Quick download — only when audio exists */}
        {hasAudio && (
          <button
            type="button"
            onClick={(e) => { e.stopPropagation(); void handleDownload(); }}
            disabled={downloading}
            title={downloading ? "Saving…" : "Download audio"}
            className="w-7 h-7 rounded-lg flex items-center justify-center transition-colors disabled:opacity-50"
            style={{
              color: downloading ? "hsl(var(--primary))" : "hsl(var(--muted-foreground))",
            }}
            onMouseEnter={(e) => { if (!downloading) e.currentTarget.style.background = "hsl(var(--surface-4))"; }}
            onMouseLeave={(e) => { e.currentTarget.style.background = "transparent"; }}
          >
            {downloading ? (
              <span className="inline-block w-2 h-2 rounded-full bg-current animate-pulse" />
            ) : (
              <Download size={13} />
            )}
          </button>
        )}

        {/* More menu */}
        <div className="relative">
          <button
            ref={btnRef}
            onClick={() => setMenuOpen((o) => !o)}
            title="More options"
            className="w-7 h-7 rounded-lg flex items-center justify-center transition-colors"
            style={{
              color: menuOpen ? "hsl(var(--foreground))" : "hsl(var(--muted-foreground))",
              background: menuOpen ? "hsl(var(--surface-4))" : "transparent",
            }}
            onMouseEnter={(e) => { e.currentTarget.style.background = "hsl(var(--surface-4))"; }}
            onMouseLeave={(e) => {
              if (!menuOpen) e.currentTarget.style.background = "transparent";
            }}
          >
            <MoreHorizontal size={14} />
          </button>

          {menuOpen && (
            <RowMenu
              recording={recording}
              playingId={playingId}
              hasAudio={hasAudio}
              onPlay={() => onPlay(recording)}
              onCopy={handleCopy}
              onCopyTranscript={handleCopyTranscript}
              onDownload={handleDownload}
              onDelete={() => onDelete(recording)}
              onClose={() => setMenuOpen(false)}
              anchorRef={btnRef}
            />
          )}
        </div>
      </div>
    </div>
  );
}

// ── Main view ─────────────────────────────────────────────────────────────────

export function HistoryView({ onDownloadSuccess, refreshKey }: { onDownloadSuccess?: (path: string) => void; refreshKey?: number }) {
  const [recordings, setRecordings] = useState<Recording[]>([]);
  const [query,      setQuery]      = useState("");
  const [refreshing, setRefreshing] = useState(false);
  const [playingId,  setPlayingId]  = useState<string | null>(null);

  const loadHistory = useCallback(async () => {
    setRefreshing(true);
    try {
      const recs = await listHistory(200);
      setRecordings(recs);
    } finally {
      setRefreshing(false);
    }
  }, []);

  useEffect(() => { void loadHistory(); }, [loadHistory, refreshKey]);

  // Filter by query (matches polished, transcript, or final_text — case-insensitive).
  const filteredRecordings = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (q.length === 0) return recordings;
    return recordings.filter((r) => {
      const hay = (r.polished ?? "") + " " + (r.transcript ?? "") + " " + (r.final_text ?? "");
      return hay.toLowerCase().includes(q);
    });
  }, [recordings, query]);

  async function handleDelete(rec: Recording) {
    stopSharedAudio();
    setPlayingId(null);
    await deleteRecording(rec.id);
    setRecordings((prev) => prev.filter((r) => r.id !== rec.id));
  }

  async function handlePlay(rec: Recording) {
    if (playingId === rec.id) {
      stopSharedAudio();
      setPlayingId(null);
      return;
    }
    try {
      const bytes = await getRecordingAudioBytes(rec.id);
      if (!bytes || bytes.length === 0) return;

      stopSharedAudio();
      const blob = new Blob([bytes as BlobPart], { type: "audio/wav" });
      const url = URL.createObjectURL(blob);
      const audio = new Audio(url);
      _activeAudio = audio;
      _activeBlobUrl = url;
      audio.onended = () => {
        setPlayingId(null);
        stopSharedAudio();
      };
      audio.onerror = () => {
        setPlayingId(null);
        stopSharedAudio();
      };
      await audio.play();
      setPlayingId(rec.id);
    } catch {
      setPlayingId(null);
      stopSharedAudio();
    }
  }

  useEffect(() => () => {
    stopSharedAudio();
  }, []);

  const timeline = groupRecordingsByDay(filteredRecordings);

  if (recordings.length === 0) {
    return (
      <div className="h-full flex items-center justify-center">
        <div className="text-center px-8">
          <div
            className="w-12 h-12 rounded-full flex items-center justify-center mx-auto mb-4"
            style={{ background: "hsl(var(--primary) / 0.15)" }}
          >
            <Clock size={20} style={{ color: "hsl(var(--chip-lime-fg))" }} />
          </div>
          <p className="text-[14px] font-semibold text-foreground mb-1">No history yet</p>
          <p className="text-[12px] text-muted-foreground max-w-xs leading-relaxed">
            Your recordings will appear here after your first session.
          </p>
        </div>
      </div>
    );
  }

  return (
    <ScrollArea className="h-full">
      <div className="p-7 pb-12 max-w-3xl mx-auto">
        <div className="flex items-start justify-between mb-5">
          <div>
            <h1 className="text-[28px] font-bold tracking-tight text-foreground leading-tight">History</h1>
            <p className="text-[13px] text-muted-foreground mt-1 tabular-nums">
              {recordings.length} recording{recordings.length !== 1 ? "s" : ""} · kept for 1 day
            </p>
          </div>
          <button
            onClick={() => void loadHistory()}
            disabled={refreshing}
            title="Refresh history"
            className="w-8 h-8 rounded-lg flex items-center justify-center transition-colors mt-1"
            style={{ background: "hsl(var(--surface-4))", color: "hsl(var(--muted-foreground))" }}
          >
            <RefreshCw size={13} className={refreshing ? "animate-spin" : ""} />
          </button>
        </div>

        {/* ── Search bar ───────────────────────────────────────── */}
        <div
          className="flex items-center gap-2 px-3 py-2 mb-6 rounded-xl"
          style={{
            background:  "hsl(var(--surface-4))",
            boxShadow:   "inset 0 0 0 1px hsl(var(--border))",
          }}
        >
          <Search size={13} className="text-muted-foreground flex-shrink-0" />
          <input
            type="text"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search recordings"
            className="flex-1 bg-transparent outline-none text-[12.5px] text-foreground placeholder:text-muted-foreground/70"
          />
          {query.length > 0 && (
            <button
              onClick={() => setQuery("")}
              className="text-muted-foreground hover:text-foreground transition-colors"
              title="Clear search"
            >
              <X size={12} />
            </button>
          )}
        </div>

        {/* No-results state when filter matches nothing */}
        {query.trim().length > 0 && filteredRecordings.length === 0 && (
          <div className="text-center py-10">
            <p className="text-[12px] text-muted-foreground">
              No recordings match "{query}".
            </p>
          </div>
        )}

        <div className="space-y-7">
          {timeline.map((group) => (
            <div key={group.label}>
              <div className="flex items-center justify-between mb-3 px-1">
                <span className="section-label">{group.label}</span>
                <span className="text-[10px] text-muted-foreground tabular-nums">
                  {group.items.length} {group.items.length === 1 ? "recording" : "recordings"}
                </span>
              </div>

              <div className="tile overflow-hidden">
                {group.items.map((rec, idx) => (
                  <React.Fragment key={rec.id}>
                    {idx > 0 && (
                      <div className="mx-5 border-t" style={{ borderColor: "hsl(var(--border))" }} />
                    )}
                    <HistoryRow
                      recording={rec}
                      playingId={playingId}
                      onPlay={handlePlay}
                      onDelete={handleDelete}
                      onDownloadSuccess={onDownloadSuccess}
                    />
                  </React.Fragment>
                ))}
              </div>
            </div>
          ))}
        </div>
      </div>
    </ScrollArea>
  );
}
