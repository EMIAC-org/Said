import { useEffect, useMemo, useState } from "react";
import { Copy, Play, Pause, Download, Check } from "lucide-react";
import { downloadRecordingAudio, listHistory } from "@/lib/invoke";
import { useAudioPlayer } from "@/lib/useAudioPlayer";
import { AppIcon, appDisplayName, fallbackAppName, useAppIdentity } from "@/components/AppIcon";
import type { AppSnapshot, Recording } from "@/types";

interface Props {
  snapshot:           AppSnapshot | null;
  onDownloadSuccess?: (path: string) => void;
  onNavigate?:        (view: string) => void;
  refreshKey?:        number;
}

/**
 * Split dashboard — pattern C from the layout proposals.
 *
 * Two columns inside the mat: a stack of insight cards on the left,
 * a day-grouped chronological recordings feed on the right.
 *
 * Layout rules:
 *   - The mat itself does NOT scroll. The right-column timeline scrolls
 *     internally so the left-column insights stay in view at all times.
 *   - The timeline only ever shows Today + Yesterday — anything older
 *     belongs in the dedicated History view (reachable via the "See all
 *     in History →" footer link).
 *   - Each recording row has a 3-dot trigger that expands inline into
 *     Copy / Listen / Download actions.
 */
export function SplitDashboard({
  snapshot, onDownloadSuccess, onNavigate, refreshKey = 0,
}: Props) {
  const [recordings, setRecordings] = useState<Recording[]>([]);

  useEffect(() => {
    let alive = true;
    listHistory(300).then((r) => { if (alive) setRecordings(r); });
    return () => { alive = false; };
  }, [refreshKey]);

  // ── Derived metrics ──────────────────────────────────────────────────────
  const totalWords    = useMemo(() => recordings.reduce((s, r) => s + r.word_count, 0), [recordings]);
  const totalSeconds  = useMemo(() => recordings.reduce((s, r) => s + r.recording_seconds, 0), [recordings]);
  const minutesSaved  = Math.max(0, Math.round((totalWords > 0 ? totalWords / 40 : 0) - totalSeconds / 60));

  const spark = useMemo(() => {
    const last = recordings.slice(0, 10).reverse();
    const max  = Math.max(1, ...last.map((r) => r.word_count));
    return last.map((r) => ({ height: Math.max(8, Math.round((r.word_count / max) * 36)) }));
  }, [recordings]);

  const apps = useMemo(() => buildAppBreakdown(recordings), [recordings]);

  // Show all recordings from the last 2 days (Today + Yesterday). The
  // dedicated History view is the place for older entries.
  const recent = useMemo(() => filterLastTwoDays(recordings), [recordings]);
  const groups = useMemo(() => groupByDay(recent), [recent]);

  return (
    <div className="h-full overflow-hidden" style={{ padding: 16 }}>

      <div
        className="grid h-full"
        style={{ gridTemplateColumns: "minmax(0,1fr) minmax(0,1.3fr)", gap: 12 }}
      >
        {/* ── LEFT: insights (does not scroll) ────────────────────────── */}
        <div className="flex flex-col gap-3 min-w-0 overflow-hidden">
          <PaceCard avgWpm={snapshot?.avg_wpm ?? 0} spark={spark} />

          <div className="grid gap-3" style={{ gridTemplateColumns: "1fr 1fr" }}>
            <MiniTile label="Words" value={totalWords.toLocaleString()} sub={`${recordings.length} dictations`} />
            <MiniTile label="Time saved" value={`${minutesSaved}`} unit="min" sub="vs. typing" />
          </div>

          <AppsCard apps={apps} />
        </div>

        {/* ── RIGHT: chronological timeline (scrolls internally) ──────── */}
        <Timeline
          groups={groups}
          total={recent.length}
          onSeeAll={() => onNavigate?.("history")}
          onDownloadSuccess={onDownloadSuccess}
        />
      </div>
    </div>
  );
}

// ── Left-column cards ───────────────────────────────────────────────────────

function PaceCard({ avgWpm, spark }: { avgWpm: number; spark: { height: number }[] }) {
  return (
    <div
      className="rounded-xl p-4"
      style={{
        background:
          "radial-gradient(80% 100% at 100% 0%, hsl(var(--primary) / 0.10), transparent 60%), hsl(var(--surface-3))",
        boxShadow: "inset 0 0 0 1px hsl(var(--glass-stroke-strong))",
      }}
    >
      <div className="text-[10px] uppercase" style={{ color: "hsl(var(--muted-foreground))", letterSpacing: "0.12em" }}>
        Avg pace · last 10 recordings
      </div>
      <div
        className="mt-1.5"
        style={{ fontSize: 36, fontWeight: 600, letterSpacing: "-0.03em", color: "hsl(var(--foreground))", lineHeight: 1.1 }}
      >
        {avgWpm}
        <span className="ml-1.5" style={{ fontSize: 14, color: "hsl(var(--muted-foreground))", fontWeight: 500 }}>
          wpm
        </span>
      </div>
      <div className="mt-1 text-[11.5px]" style={{ color: "hsl(var(--muted-foreground))" }}>
        {avgWpm > 40 ? `+${Math.round(((avgWpm - 40) / 40) * 100)}% vs typing` : "Typing baseline"}
      </div>
      <div className="mt-3 flex items-end gap-[3px]" style={{ height: 36 }}>
        {spark.length === 0 ? (
          <div className="text-[10.5px] self-center" style={{ color: "hsl(var(--muted-foreground))" }}>
            Need 10 recordings for the trend
          </div>
        ) : (
          spark.map((b, i) => (
            <div
              key={i}
              style={{
                flex: 1,
                height: b.height,
                borderRadius: 2,
                background:
                  i === spark.length - 1
                    ? "hsl(var(--primary) / 0.55)"
                    : i % 2 === 0
                      ? "hsl(0 0% 100% / 0.07)"
                      : "hsl(var(--primary) / 0.40)",
              }}
            />
          ))
        )}
      </div>
    </div>
  );
}

function MiniTile({ label, value, unit, sub }: { label: string; value: string; unit?: string; sub: string }) {
  return (
    <div
      className="rounded-xl p-3.5"
      style={{
        background: "hsl(var(--surface-3))",
        boxShadow: "inset 0 0 0 1px hsl(var(--glass-stroke))",
      }}
    >
      <div className="text-[10px] uppercase" style={{ color: "hsl(var(--muted-foreground))", letterSpacing: "0.10em" }}>
        {label}
      </div>
      <div className="mt-1" style={{ fontSize: 18, fontWeight: 600, letterSpacing: "-0.02em", color: "hsl(var(--foreground))" }}>
        {value}
        {unit && (
          <span className="ml-1" style={{ fontSize: 11, color: "hsl(var(--muted-foreground))", fontWeight: 500 }}>
            {unit}
          </span>
        )}
      </div>
      <div className="mt-0.5 text-[10.5px]" style={{ color: "hsl(var(--muted-foreground))" }}>
        {sub}
      </div>
    </div>
  );
}

interface AppSummary {
  key: string | null;
  label: string;
  count: number;
}

function AppsCard({ apps }: { apps: AppSummary[] }) {
  return (
    <div
      className="rounded-xl p-4 min-h-0 overflow-hidden"
      style={{
        background: "hsl(var(--surface-3))",
        boxShadow: "inset 0 0 0 1px hsl(var(--glass-stroke))",
      }}
    >
      <div className="text-[12px] font-semibold mb-2.5" style={{ color: "hsl(var(--foreground))" }}>
        Fixes by app
      </div>
      {apps.length === 0 ? (
        <p className="text-[11.5px]" style={{ color: "hsl(var(--muted-foreground))" }}>
          We'll track which apps you dictate into the most.
        </p>
      ) : apps.map((app) => <AppSummaryRow key={app.key ?? app.label} app={app} />)}
    </div>
  );
}

function AppSummaryRow({ app }: { app: AppSummary }) {
  const identity = useAppIdentity(app.key);
  const label = app.key ? appDisplayName(app.key, identity) : app.label;

  return (
    <div
      className="grid items-center"
      style={{ gridTemplateColumns: "22px 1fr 44px", gap: 10, padding: "5px 0", fontSize: 11.5 }}
    >
      <AppIcon appKey={app.key} label={label} size={22} radius={6} fallbackSize={12} />
      <span className="truncate" style={{ color: "hsl(var(--foreground))" }}>{label}</span>
      <span
        className="text-right"
        style={{ color: "hsl(var(--muted-foreground))", fontSize: 11, fontFamily: "ui-monospace, SF Mono, monospace" }}
      >
        {app.count.toLocaleString()}
      </span>
    </div>
  );
}

// ── Right-column timeline ───────────────────────────────────────────────────

function Timeline({
  groups, total, onSeeAll, onDownloadSuccess,
}: {
  groups: { label: string; items: Recording[] }[];
  total: number;
  onSeeAll: () => void;
  onDownloadSuccess?: (path: string) => void;
}) {
  const { playingId, play } = useAudioPlayer();

  return (
    <div
      className="rounded-xl flex flex-col min-w-0 min-h-0"
      style={{
        background: "hsl(var(--surface-3))",
        boxShadow: "inset 0 0 0 1px hsl(var(--glass-stroke))",
      }}
    >
      {/* Header — flex-shrink-0 keeps it visible while body scrolls */}
      <div
        className="flex items-center justify-between flex-shrink-0 px-4 pt-4 pb-3"
        style={{ borderBottom: "1px solid hsl(var(--border) / 0.5)" }}
      >
        <div className="text-[12.5px] font-semibold" style={{ color: "hsl(var(--foreground))" }}>
          Recent dictations
        </div>
        <span
          className="text-[10.5px]"
          style={{ color: "hsl(var(--muted-foreground))" }}
        >
          Last 2 days · {total}
        </span>
      </div>

      {/* Scrollable body — card stack */}
      <div className="flex-1 min-h-0 overflow-y-auto px-3 pb-2">
        {groups.length === 0 ? (
          <p className="text-[12.5px] py-6 text-center" style={{ color: "hsl(var(--muted-foreground))" }}>
            Nothing yet today or yesterday. Your dictations will land here.
          </p>
        ) : (
          groups.map((g) => (
            <div key={g.label}>
              <div
                className="px-1 mt-3 mb-1.5 text-[10px] uppercase"
                style={{ color: "hsl(var(--muted-foreground))", letterSpacing: "0.12em" }}
              >
                {g.label}
              </div>
              {g.items.map((r) => (
                <RecordingCard
                  key={r.id}
                  rec={r}
                  isPlaying={playingId === r.id}
                  onPlay={() => play(r.id, r.audio_id)}
                  onDownloadSuccess={onDownloadSuccess}
                />
              ))}
            </div>
          ))
        )}
      </div>

      {/* Footer link — older entries live in the dedicated History view */}
      <div
        className="flex-shrink-0 px-4 py-2.5"
        style={{ borderTop: "1px solid hsl(var(--border) / 0.5)" }}
      >
        <button
          onClick={onSeeAll}
          className="text-[11.5px] font-medium transition-colors"
          style={{ color: "hsl(var(--muted-foreground))", background: "transparent", border: 0, cursor: "pointer" }}
          onMouseEnter={(e) => (e.currentTarget.style.color = "hsl(var(--foreground))")}
          onMouseLeave={(e) => (e.currentTarget.style.color = "hsl(var(--muted-foreground))")}
        >
          See full history →
        </button>
      </div>
    </div>
  );
}

// ── Recording card — Variant 1 "Card stack" from the timeline refactor ──────
// Each row is its own quiet card. Single-line clamp. Word count is shown by
// default; on row hover it fades out and the action trio (Copy / Listen /
// Download) fades in. While a row is the active playback target, or the user
// just clicked Copy, actions stay visible so the feedback is legible.

function RecordingCard({
  rec, isPlaying, onPlay, onDownloadSuccess,
}: {
  rec:        Recording;
  isPlaying:  boolean;
  onPlay:     () => void;
  onDownloadSuccess?: (path: string) => void;
}) {
  const [hover, setHover]             = useState(false);
  const [copied, setCopied]           = useState(false);
  const [downloading, setDownloading] = useState(false);
  const appIdentity = useAppIdentity(rec.target_app);
  const appName = appDisplayName(rec.target_app, appIdentity);

  async function copy(e: React.MouseEvent) {
    e.stopPropagation();
    try {
      await navigator.clipboard.writeText(rec.polished);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1400);
    } catch { /* ignore */ }
  }

  async function download(e: React.MouseEvent) {
    e.stopPropagation();
    if (!rec.audio_id || downloading) return;
    setDownloading(true);
    try {
      const path = await downloadRecordingAudio(rec.id, audioFilename(rec));
      if (path) onDownloadSuccess?.(path);
    } finally {
      setDownloading(false);
    }
  }

  const canPlay     = Boolean(rec.audio_id);
  const canDownload = Boolean(rec.audio_id);
  // Keep actions visible after click feedback so user sees the result.
  const showActions = hover || isPlaying || copied || downloading;

  return (
    <div
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
      className="transition-all"
      style={{
        padding: "11px 14px",
        marginBottom: 6,
        borderRadius: 10,
        background: hover ? "hsl(var(--surface-hover))" : "hsl(var(--surface-3))",
        boxShadow: hover
          ? "inset 0 0 0 1px hsl(var(--glass-stroke-strong))"
          : "inset 0 0 0 1px hsl(var(--glass-stroke))",
      }}
    >
      {/* Meta row — time · app · (word count ⇄ actions) */}
      <div className="flex items-center gap-2 mb-1.5" style={{ minHeight: 18 }}>
        <span
          style={{ fontSize: 10.5, color: "hsl(var(--muted-foreground))", fontFamily: "ui-monospace, SF Mono, monospace" }}
        >
          {timeOfDay(rec.timestamp_ms)}
        </span>
        {rec.target_app && (
          <span
            className="inline-flex items-center gap-1.5 min-w-0"
            style={{
              fontSize: 10,
              color: "hsl(var(--muted-foreground))",
              maxWidth: 120,
            }}
            title={appName}
          >
            <AppIcon appKey={rec.target_app} label={appName} size={16} radius={4} fallbackSize={10} />
            <span className="truncate">{appName}</span>
          </span>
        )}

        {/* Right slot — word count by default, actions on hover/active */}
        <div className="ml-auto flex items-center" style={{ position: "relative", minHeight: 22 }}>
          <span
            className="whitespace-nowrap"
            style={{
              fontSize: 11,
              color: "hsl(var(--muted-foreground))",
              fontFamily: "ui-monospace, SF Mono, monospace",
              opacity: showActions ? 0 : 1,
              transition: "opacity 0.12s ease",
              pointerEvents: showActions ? "none" : "auto",
            }}
          >
            {rec.word_count} w
          </span>
          <div
            className="absolute right-0 top-0 flex items-center gap-1"
            style={{
              opacity: showActions ? 1 : 0,
              transition: "opacity 0.12s ease",
              pointerEvents: showActions ? "auto" : "none",
            }}
          >
            <ActionButton title={copied ? "Copied" : "Copy text"} onClick={copy} active={copied}>
              {copied ? <Check size={12} strokeWidth={2.6} /> : <Copy size={12} />}
            </ActionButton>
            <ActionButton
              title={canPlay ? (isPlaying ? "Pause" : "Listen") : "No audio"}
              onClick={canPlay ? onPlay : undefined}
              disabled={!canPlay}
              active={isPlaying}
            >
              {isPlaying ? <Pause size={12} /> : <Play size={12} />}
            </ActionButton>
            <ActionButton
              title={canDownload ? "Download audio" : "No audio"}
              onClick={canDownload ? download : undefined}
              disabled={!canDownload || downloading}
            >
              <Download size={12} />
            </ActionButton>
          </div>
        </div>
      </div>

      {/* Single-line preview — title attribute holds the full text */}
      <div
        className="text-[13px] leading-snug"
        style={{
          color: "hsl(var(--foreground))",
          display: "-webkit-box",
          WebkitLineClamp: 1,
          WebkitBoxOrient: "vertical",
          overflow: "hidden",
          wordBreak: "break-word",
        }}
        title={rec.polished}
      >
        {rec.polished}
      </div>
    </div>
  );
}

function ActionButton({
  title, onClick, children, active, disabled,
}: {
  title:     string;
  onClick?:  (e: React.MouseEvent) => void;
  children:  React.ReactNode;
  active?:   boolean;
  disabled?: boolean;
}) {
  return (
    <button
      title={title}
      onClick={onClick}
      disabled={disabled || !onClick}
      className="grid place-items-center transition-all"
      style={{
        width: 22, height: 22,
        borderRadius: 5,
        background: active ? "hsl(var(--primary) / 0.18)" : "hsl(0 0% 100% / 0.05)",
        color: active ? "hsl(var(--primary))" : "hsl(var(--foreground))",
        border: 0,
        cursor: disabled || !onClick ? "not-allowed" : "pointer",
        opacity: disabled || !onClick ? 0.4 : 1,
      }}
      onMouseEnter={(e) => {
        if (disabled || !onClick) return;
        if (!active) e.currentTarget.style.background = "hsl(0 0% 100% / 0.10)";
      }}
      onMouseLeave={(e) => {
        if (disabled || !onClick) return;
        if (!active) e.currentTarget.style.background = "hsl(0 0% 100% / 0.05)";
      }}
    >
      {children}
    </button>
  );
}

// ── Helpers ──────────────────────────────────────────────────────────────────

function buildAppBreakdown(recs: Recording[]): AppSummary[] {
  const counts = new Map<string, number>();
  for (const r of recs) {
    const k = r.target_app && r.target_app.trim() ? r.target_app : "Unknown";
    counts.set(k, (counts.get(k) ?? 0) + 1);
  }
  const sorted = Array.from(counts.entries()).sort((a, b) => b[1] - a[1]);
  const top = sorted.slice(0, 4).map(([key, count]) => ({
    key: key === "Unknown" ? null : key,
    label: key === "Unknown" ? "Unknown app" : fallbackAppName(key),
    count,
  }));
  const restCount = sorted.slice(4).reduce((s, [, c]) => s + c, 0);
  if (restCount > 0) top.push({ key: null, label: "Other apps", count: restCount });
  return top;
}

function filterLastTwoDays(recs: Recording[]): Recording[] {
  const now = new Date();
  const todayStart = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
  const cutoff = todayStart - 24 * 3600 * 1000; // start of yesterday
  return recs.filter((r) => r.timestamp_ms >= cutoff);
}

function groupByDay(recs: Recording[]): { label: string; items: Recording[] }[] {
  const today = new Date();
  const todayKey = `${today.getFullYear()}-${today.getMonth()}-${today.getDate()}`;
  const ystKey = (() => {
    const y = new Date(today); y.setDate(y.getDate() - 1);
    return `${y.getFullYear()}-${y.getMonth()}-${y.getDate()}`;
  })();

  const groups = new Map<string, { label: string; items: Recording[] }>();
  for (const r of recs) {
    const d = new Date(r.timestamp_ms);
    const key = `${d.getFullYear()}-${d.getMonth()}-${d.getDate()}`;
    const label = key === todayKey ? "Today" : key === ystKey ? "Yesterday" : "Earlier";
    const g = groups.get(key) ?? { label, items: [] };
    g.items.push(r);
    groups.set(key, g);
  }
  return Array.from(groups.values());
}

function timeOfDay(ms: number): string {
  const d = new Date(ms);
  return d.toLocaleTimeString("en-US", { hour: "2-digit", minute: "2-digit", hour12: false });
}

function audioFilename(rec: Recording): string {
  const d   = new Date(rec.timestamp_ms);
  const pad = (n: number) => String(n).padStart(2, "0");
  const stamp = `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}-${pad(d.getHours())}${pad(d.getMinutes())}`;
  return `said-${stamp}-${rec.word_count}-words.wav`;
}
