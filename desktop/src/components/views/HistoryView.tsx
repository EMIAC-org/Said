import React, { useEffect, useMemo, useRef, useState } from "react";
import { Clock, Copy, Play, Pause, Trash2, MoreHorizontal, Check, Search, X, Download } from "lucide-react";
import { ScrollArea } from "@/components/ui/scroll-area";
import { groupHistory } from "@/types";
import type { Recording } from "@/types";
import { deleteRecording, listHistory, downloadRecordingAudio as saveRecordingAudio } from "@/lib/invoke";
import { useAudioPlayer } from "@/lib/useAudioPlayer";

// ── Download helper ───────────────────────────────────────────────────────────

/** Build a friendly filename: "said-2026-05-03-1430-12-words.wav". */
function audioFilename(recording: Recording): string {
  const d     = new Date(recording.timestamp_ms);
  const pad   = (n: number) => String(n).padStart(2, "0");
  const stamp = `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}-${pad(d.getHours())}${pad(d.getMinutes())}`;
  return `said-${stamp}-${recording.word_count}-words.wav`;
}

/** Ask Tauri to save the WAV to Downloads. Returns the saved path on success. */
async function downloadRecordingAudio(recording: Recording): Promise<string | null> {
  if (!recording.audio_id) return null;
  return await saveRecordingAudio(recording.id, audioFilename(recording));
}

// ── Context menu ──────────────────────────────────────────────────────────────

interface MenuProps {
  recording:   Recording;
  playingId:   string | null;
  onPlay:      () => void;
  onCopy:      () => void;
  onDownload:  () => void;
  onDelete:    () => void;
  onClose:     () => void;
  anchorRef:   React.RefObject<HTMLButtonElement | null>;
}

function RowMenu({ recording, playingId, onPlay, onCopy, onDownload, onDelete, onClose, anchorRef }: MenuProps) {
  const menuRef  = useRef<HTMLDivElement>(null);
  const isPlaying = playingId === recording.id;
  const hasAudio  = !!recording.audio_id;

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
      {item(<Copy size={13} />, "Copy text", onCopy)}
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

// Word-count threshold above which a recording is collapsed with a Read-more
const TRUNCATE_WORD_LIMIT = 30;

function HistoryRow({ recording, playingId, onPlay, onDelete, onDownloadSuccess }: RowProps) {
  const [menuOpen,    setMenuOpen]    = useState(false);
  const [copied,      setCopied]      = useState(false);
  const [expanded,    setExpanded]    = useState(false);
  const [downloading, setDownloading] = useState(false);
  const btnRef = useRef<HTMLButtonElement>(null);

  async function handleDownload() {
    if (!recording.audio_id || downloading) return;
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
    setCopied(true);
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
      <div className="flex-shrink-0 flex items-center gap-1 opacity-0 group-hover:opacity-100 transition-opacity">
        {/* Quick copy */}
        <button
          onClick={handleCopy}
          title="Copy text"
          className="w-7 h-7 rounded-lg flex items-center justify-center transition-colors"
          style={{ color: copied ? "hsl(var(--chip-lime-fg))" : "hsl(var(--muted-foreground))" }}
          onMouseEnter={(e) => { e.currentTarget.style.background = "hsl(var(--surface-4))"; }}
          onMouseLeave={(e) => { e.currentTarget.style.background = "transparent"; }}
        >
          {copied ? <Check size={13} /> : <Copy size={13} />}
        </button>

        {/* Quick play — only when audio exists */}
        {recording.audio_id && (
          <button
            onClick={() => onPlay(recording)}
            title={isPlaying ? "Pause" : "Play"}
            className="w-7 h-7 rounded-lg flex items-center justify-center transition-colors"
            style={{ color: isPlaying ? "hsl(var(--chip-lime-fg))" : "hsl(var(--muted-foreground))" }}
            onMouseEnter={(e) => { e.currentTarget.style.background = "hsl(var(--surface-4))"; }}
            onMouseLeave={(e) => { e.currentTarget.style.background = "transparent"; }}
          >
            {isPlaying ? <Pause size={13} /> : <Play size={13} />}
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
              onPlay={() => onPlay(recording)}
              onCopy={handleCopy}
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

export function HistoryView({ onDownloadSuccess }: { onDownloadSuccess?: (path: string) => void }) {
  const [recordings, setRecordings] = useState<Recording[]>([]);
  const [query,      setQuery]      = useState("");
  const { playingId, play, stop }   = useAudioPlayer();

  useEffect(() => {
    listHistory(200).then(setRecordings);
  }, []);

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
    stop();
    await deleteRecording(rec.id);
    setRecordings((prev) => prev.filter((r) => r.id !== rec.id));
  }

  function handlePlay(rec: Recording) {
    play(rec.id, rec.audio_id);
  }

  const items = filteredRecordings.map((r) => ({
    timestamp_ms:      r.timestamp_ms,
    polished:          r.polished,
    word_count:        r.word_count,
    recording_seconds: r.recording_seconds,
    model:             r.model_used,
    transcribe_ms:     r.transcribe_ms ?? 0,
    embed_ms:          r.embed_ms ?? 0,
    polish_ms:         r.polish_ms ?? 0,
    audio_id:          r.audio_id,
  }));
  const timeline = groupHistory(items);

  // Map group index back to recordings for easy lookup
  const recByTs = new Map(filteredRecordings.map((r) => [r.timestamp_ms, r]));

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
        <div className="mb-5">
          <h1 className="text-[28px] font-bold tracking-tight text-foreground leading-tight">History</h1>
          <p className="text-[13px] text-muted-foreground mt-1 tabular-nums">
            {recordings.length} recording{recordings.length !== 1 ? "s" : ""} · kept for 1 day
          </p>
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
                {group.items.map((item, idx) => {
                  const rec = recByTs.get(item.timestamp_ms);
                  if (!rec) return null;
                  return (
                    <React.Fragment key={rec.id}>
                      {idx > 0 && (
                        <div className="mx-5 border-t" style={{ borderColor: "hsl(var(--surface-3))" }} />
                      )}
                      <HistoryRow
                        recording={rec}
                        playingId={playingId}
                        onPlay={handlePlay}
                        onDelete={handleDelete}
                        onDownloadSuccess={onDownloadSuccess}
                      />
                    </React.Fragment>
                  );
                })}
              </div>
            </div>
          ))}
        </div>
      </div>
    </ScrollArea>
  );
}
