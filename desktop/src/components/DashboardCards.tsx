import React, { useState, useEffect, useRef } from "react";
import {
  ChevronRight,
  Filter, Play, Pause, Check, Copy, Download,
  ChevronDown as CaretDown,
} from "lucide-react";
import { useAudioPlayer } from "@/lib/useAudioPlayer";
import { downloadRecordingAudio } from "@/lib/invoke";
import type { AppSnapshot, Recording } from "@/types";

/* ════════════════════════════════════════════════════════════════════════════
   Sentinel-inspired stat tile primitives.
   All hero cards share: bold title, ··· menu, tiny grey context label,
   GIANT tabular number, and a small green ▲ delta chip.
   ════════════════════════════════════════════════════════════════════════════ */

function DeltaChip({
  value, suffix = "%", neutral, color = "mint",
}: {
  value: number;
  suffix?: string;
  neutral?: boolean;
  color?: "mint" | "blue" | "amber";
}) {
  const isPositive = value > 0;
  const isZero     = value === 0 || neutral;
  const sign       = isZero ? "" : isPositive ? "+" : "";
  const colorMap = {
    mint:  { bg: "hsl(var(--chip-mint-bg))",  fg: "hsl(var(--chip-mint-fg))"  },
    blue:  { bg: "hsl(var(--chip-blue-bg))",  fg: "hsl(var(--chip-blue-fg))"  },
    amber: { bg: "hsl(var(--chip-amber-bg))", fg: "hsl(var(--chip-amber-fg))" },
  };
  const c = isZero
    ? { bg: "hsl(var(--surface-4))", fg: "hsl(var(--muted-foreground))" }
    : colorMap[color];
  return (
    <span
      className="inline-flex items-center px-2 py-0.5 rounded-md text-[11px] font-semibold tabular-nums"
      style={{ background: c.bg, color: c.fg, lineHeight: 1.4 }}
    >
      {sign}{value.toLocaleString()}{suffix}
    </span>
  );
}

/* ════════════════════════════════════════════════════════════════════════════
   StatTile — uniform compact card body.
   Layout: title (+optional status) + ··· menu / subtitle / NUMBER + chip
   No `mt-auto`, no `flex-1` — natural top-down flow so all cards in a
   row size to the same content height (no awkward empty space).
   ════════════════════════════════════════════════════════════════════════════ */

interface StatTileProps {
  title:     string;
  subtitle:  string;
  value:     React.ReactNode;       // big number, tabular-nums leaf
  delta?:    React.ReactNode;       // <DeltaChip /> or null
  status?:   { label: string; pulse?: boolean } | null;
}

function StatTile({ title, subtitle, value, delta, status }: StatTileProps) {
  return (
    <div className="panel px-5 pt-4 pb-5">
      {/* Title row */}
      <div className="flex items-center gap-2 min-w-0">
        <p className="text-[13px] font-bold tracking-tight truncate"
           style={{ color: "hsl(var(--foreground))" }}>
          {title}
        </p>
        {status && (
          <span
            className="inline-flex items-center gap-1 px-1.5 py-0.5 rounded-md text-[9px] font-bold tabular-nums flex-shrink-0"
            style={{
              background: "hsl(var(--primary) / 0.14)",
              color:      "hsl(var(--primary))",
            }}
          >
            <span
              className={`inline-block w-1 h-1 rounded-full ${status.pulse ? "animate-pulse" : ""}`}
              style={{ background: "currentColor" }}
            />
            {status.label}
          </span>
        )}
      </div>

      {/* Subtitle — tight under title */}
      <p className="text-[11.5px] mt-0.5" style={{ color: "hsl(var(--muted-foreground))" }}>
        {subtitle}
      </p>

      {/* Number + delta — generous top space, no extra padding below */}
      <div className="flex items-baseline gap-2 mt-4 flex-wrap">
        <span
          className="font-bold tabular-nums leading-none tracking-tight"
          style={{
            fontSize: 28,
            color:    "hsl(var(--foreground))",
            letterSpacing: "-0.02em",
          }}
        >
          {value}
        </span>
        {delta}
      </div>
    </div>
  );
}


/* ════════════════════════════════════════════════════════════════════════════
   2) DonutCard — total words polished. Donut visualization dropped so the
      card matches the others' compact natural height.
   ════════════════════════════════════════════════════════════════════════════ */

export function DonutCard({
  snapshot,
  isProcessing,
  isRecording,
}: {
  snapshot:      AppSnapshot | null;
  isProcessing?: boolean;
  isRecording?:  boolean;
}) {
  const history    = snapshot?.history ?? [];
  const totalWords = snapshot?.total_words ?? 0;
  const goal       = 50_000;
  const pct        = Math.min(100, (totalWords / goal) * 100);

  const start = new Date();
  start.setHours(0, 0, 0, 0);
  const todayWords = history
    .filter((h) => h.timestamp_ms >= start.getTime())
    .reduce((s, h) => s + h.word_count, 0);

  const status = isRecording  ? { label: "REC",       pulse: true }
               : isProcessing ? { label: "POLISHING", pulse: true }
               : null;

  return (
    <StatTile
      title="Words polished"
      subtitle={`${Math.round(pct)}% of ${(goal / 1000).toFixed(0)}k goal`}
      value={totalWords.toLocaleString()}
      delta={<DeltaChip value={todayWords} suffix="" neutral={todayWords === 0} />}
      status={status}
    />
  );
}

/* ════════════════════════════════════════════════════════════════════════════
   3) TimeSavedCard — minutes saved by dictating instead of typing.
   ════════════════════════════════════════════════════════════════════════════ */

const TYPING_WPM = 40;

function formatMinutes(min: number): { value: string; unit: string } {
  if (min < 1)    return { value: "0", unit: "min" };
  if (min < 60)   return { value: `${min}`, unit: "min" };
  const h = Math.floor(min / 60);
  const m = min % 60;
  return { value: m === 0 ? `${h}` : `${h}h ${m}`, unit: m === 0 ? "h" : "m" };
}

export function TimeSavedCard({ snapshot }: { snapshot: AppSnapshot | null }) {
  const history    = snapshot?.history ?? [];
  const dictWpm    = snapshot?.avg_wpm ?? 0;
  const totalWords = snapshot?.total_words ?? 0;

  const weekStart      = Date.now() - 7 * 86_400_000;
  const wordsThisWeek  = history
    .filter((h) => h.timestamp_ms >= weekStart)
    .reduce((s, h) => s + h.word_count, 0);

  const useWeek          = wordsThisWeek > 0;
  const wordsForCalc     = useWeek ? wordsThisWeek : totalWords;
  const effectiveDictWpm = dictWpm > 0 ? dictWpm : 120;
  const minutesSaved     = Math.max(
    0,
    Math.round(wordsForCalc / TYPING_WPM - wordsForCalc / effectiveDictWpm),
  );
  const multiplier = effectiveDictWpm / TYPING_WPM;
  const f          = formatMinutes(minutesSaved);

  return (
    <StatTile
      title="Time saved"
      subtitle={useWeek ? "Last 7 days, vs typing at 40 WPM" : "All time, vs typing at 40 WPM"}
      value={
        <>
          {f.value}
          <span className="text-[13px] ml-1"
                style={{ color: "hsl(var(--muted-foreground))", fontWeight: 600 }}>
            {f.unit}
          </span>
        </>
      }
      delta={<DeltaChip value={Number(multiplier.toFixed(1))} suffix="×" neutral={multiplier <= 1} />}
    />
  );
}

/* ════════════════════════════════════════════════════════════════════════════
   4) PaceCard — average WPM, showing dictation speed at a glance.
   ════════════════════════════════════════════════════════════════════════════ */

export function PaceCard({ snapshot }: { snapshot: AppSnapshot | null }) {
  const wpm = snapshot?.avg_wpm ?? 0;
  // Delta vs typical typing speed (40 WPM) — % faster
  const deltaPct = wpm > 0 ? Math.round(((wpm - TYPING_WPM) / TYPING_WPM) * 100) : 0;

  return (
    <StatTile
      title="Avg pace"
      subtitle="Rolling 10-recording WPM"
      value={
        <>
          {wpm || "—"}
          {wpm > 0 && (
            <span className="text-[13px] ml-1"
                  style={{ color: "hsl(var(--muted-foreground))", fontWeight: 600 }}>
              WPM
            </span>
          )}
        </>
      }
      delta={wpm > 0 ? <DeltaChip value={deltaPct} /> : null}
    />
  );
}

/* ════════════════════════════════════════════════════════════════════════════
   4) RecordingsTable — clean white table, mint accents, dotted dividers.
   ════════════════════════════════════════════════════════════════════════════ */

function modelLabel(model: string): string {
  if (model.includes("mini"))   return "Fast";
  if (model.includes("claude")) return "Claude";
  if (model.includes("gemini")) return "Gemini";
  return "Smart";
}

function relTime(ms: number): string {
  const diff = Date.now() - ms;
  const min  = Math.floor(diff / 60_000);
  if (min < 1)  return "just now";
  if (min < 60) return `${min}m ago`;
  const hr   = Math.floor(min / 60);
  if (hr < 24) return `${hr}h ago`;
  const d = Math.floor(hr / 24);
  if (d === 1) return "yesterday";
  if (d < 7)   return `${d}d ago`;
  return new Date(ms).toLocaleDateString("en-US", { month: "short", day: "numeric" });
}

type RecordingsFilter = "all" | "today" | "week" | "month";
const FILTER_LABEL: Record<RecordingsFilter, string> = {
  all:   "All time",
  today: "Today",
  week:  "This week",
  month: "This month",
};

function audioFilename(recording: Recording): string {
  const d     = new Date(recording.timestamp_ms);
  const pad   = (n: number) => String(n).padStart(2, "0");
  const stamp = `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}-${pad(d.getHours())}${pad(d.getMinutes())}`;
  return `said-${stamp}-${recording.word_count}-words.wav`;
}

export function RecordingsTable({
  recordings, onSeeAll, onDownloadSuccess,
}: {
  recordings: Recording[];
  onSeeAll:   () => void;
  onDownloadSuccess?: (path: string) => void;
}) {
  const { playingId, play } = useAudioPlayer();

  const [filter,    setFilter]    = useState<RecordingsFilter>("all");
  const [filterOpen, setFilterOpen] = useState(false);
  const filterRef = useRef<HTMLDivElement>(null);

  // Outside-click + escape close
  useEffect(() => {
    if (!filterOpen) return;
    const onDown = (e: MouseEvent) => {
      if (filterRef.current && !filterRef.current.contains(e.target as Node)) setFilterOpen(false);
    };
    const onEsc = (e: KeyboardEvent) => { if (e.key === "Escape") setFilterOpen(false); };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onEsc);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onEsc);
    };
  }, [filterOpen]);

  // Apply the active filter — narrow the recordings list before slicing
  const filtered = (() => {
    if (filter === "all") return recordings;
    const now = new Date();
    let start: number;
    if (filter === "today") {
      const t = new Date(now); t.setHours(0, 0, 0, 0);
      start = t.getTime();
    } else if (filter === "week") {
      start = now.getTime() - 7 * 86_400_000;
    } else {
      start = now.getTime() - 30 * 86_400_000;
    }
    return recordings.filter((r) => r.timestamp_ms >= start);
  })();

  const items = filtered.slice(0, 4);

  return (
    <div className="panel p-5">
      <div className="flex items-center justify-between mb-4">
        <div className="flex items-center gap-2.5">
          <h3 className="text-[15px] font-bold tracking-tight"
              style={{ color: "hsl(var(--foreground))" }}>
            Recordings list
          </h3>
          <span style={{ color: "hsl(var(--muted-foreground) / 0.4)" }}>|</span>

          {/* Filter dropdown */}
          <div ref={filterRef} className="relative">
            <button
              onClick={() => setFilterOpen((o) => !o)}
              className="flex items-center gap-1 text-[12.5px] font-medium transition-colors"
              style={{
                color: filter === "all"
                  ? "hsl(var(--muted-foreground))"
                  : "hsl(var(--primary))",
              }}
              onMouseEnter={(e) => {
                if (filter === "all") e.currentTarget.style.color = "hsl(var(--foreground))";
              }}
              onMouseLeave={(e) => {
                if (filter === "all") e.currentTarget.style.color = "hsl(var(--muted-foreground))";
              }}
            >
              <Filter size={12} />
              {filter === "all" ? "Filter" : FILTER_LABEL[filter]}
              <CaretDown
                size={10}
                style={{ transition: "transform 0.15s", transform: filterOpen ? "rotate(180deg)" : "none" }}
              />
            </button>
            {filterOpen && (
              <div
                className="absolute left-0 top-full mt-1 z-30 rounded-md py-1 min-w-[140px]"
                style={{
                  background: "hsl(var(--surface-3))",
                  boxShadow:
                    "inset 0 0 0 1px hsl(var(--border)), 0 8px 24px hsl(0 0% 0% / 0.12)",
                }}
              >
                {(Object.keys(FILTER_LABEL) as RecordingsFilter[]).map((k) => {
                  const active = filter === k;
                  return (
                    <button
                      key={k}
                      onClick={() => { setFilter(k); setFilterOpen(false); }}
                      className="w-full flex items-center justify-between gap-2 px-3 py-1.5 text-[12px] font-medium text-left transition-colors"
                      style={{
                        color:      active ? "hsl(var(--primary))" : "hsl(var(--foreground))",
                        background: active ? "hsl(var(--primary) / 0.08)" : "transparent",
                      }}
                      onMouseEnter={(e) => {
                        if (!active) e.currentTarget.style.background = "hsl(var(--surface-hover))";
                      }}
                      onMouseLeave={(e) => {
                        if (!active) e.currentTarget.style.background = "transparent";
                      }}
                    >
                      {FILTER_LABEL[k]}
                      {active && <Check size={11} strokeWidth={2.5} />}
                    </button>
                  );
                })}
              </div>
            )}
          </div>
        </div>
        <button
          onClick={onSeeAll}
          className="flex items-center gap-1 text-[12.5px] font-medium transition-colors"
          style={{ color: "hsl(var(--muted-foreground))" }}
          onMouseEnter={(e) => { e.currentTarget.style.color = "hsl(var(--foreground))"; }}
          onMouseLeave={(e) => { e.currentTarget.style.color = "hsl(var(--muted-foreground))"; }}
        >
          See all
          <ChevronRight size={12} />
        </button>
      </div>

      {/* Column headers */}
      <div
        className="grid items-center gap-4 py-2 text-[10.5px] font-semibold uppercase tracking-wider"
        style={{
          gridTemplateColumns: "1fr 110px 110px 100px 104px",
          color: "hsl(var(--muted-foreground))",
        }}
      >
        <span>Polished text</span>
        <span>Status</span>
        <span>When</span>
        <span className="text-right">Words</span>
        <span className="text-right">Audio</span>
      </div>

      {items.length === 0 ? (
        <div className="py-10 text-center">
          <p className="text-[12.5px]" style={{ color: "hsl(var(--muted-foreground))" }}>
            {filter === "all" ? (
              <>Press <span className="font-semibold" style={{ color: "hsl(var(--foreground))" }}>⇪ Caps Lock</span>
              {" "}to record. Recent recordings appear here.</>
            ) : (
              <>No recordings <span style={{ color: "hsl(var(--foreground))", fontWeight: 600 }}>
                {FILTER_LABEL[filter].toLowerCase()}</span>.{" "}
              <button
                onClick={() => setFilter("all")}
                className="underline"
                style={{ color: "hsl(var(--primary))" }}
              >
                Show all
              </button>
              </>
            )}
          </p>
        </div>
      ) : (
        <div className="flex flex-col">
          {items.map((rec, i) => (
            <Row
              key={rec.id}
              rec={rec}
              live={i === 0}
              last={i === items.length - 1}
              isPlaying={playingId === rec.id}
              onPlay={() => play(rec.id, rec.audio_id)}
              onDownloadSuccess={onDownloadSuccess}
            />
          ))}
        </div>
      )}
    </div>
  );
}

function Row({
  rec, live, last, isPlaying, onPlay, onDownloadSuccess,
}: {
  rec:        Recording;
  live:       boolean;
  last:       boolean;
  isPlaying:  boolean;
  onPlay:     () => void;
  onDownloadSuccess?: (path: string) => void;
}) {
  const title    = rec.polished;
  const model    = modelLabel(rec.model_used);

  const isRecent = Date.now() - rec.timestamp_ms < 5 * 60_000;
  const chipBg   = isPlaying
                   ? "hsl(var(--chip-mint-bg))"
                   : "hsl(var(--chip-mint-bg))";
  const chipFg   = "hsl(var(--chip-mint-fg))";
  const chipText = isPlaying ? "Playing…" : live ? "Latest" : isRecent ? "Recent" : "Polished";

  const [copied, setCopied] = useState(false);
  const [downloading, setDownloading] = useState(false);
  const handleCopy = async (e: React.MouseEvent) => {
    e.stopPropagation();
    try {
      await navigator.clipboard.writeText(rec.polished);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    } catch { /* ignore */ }
  };

  const canPlay = Boolean(rec.audio_id);
  const handleDownload = async () => {
    if (!rec.audio_id || downloading) return;
    setDownloading(true);
    try {
      const savedPath = await downloadRecordingAudio(rec.id, audioFilename(rec));
      if (savedPath) onDownloadSuccess?.(savedPath);
    } finally {
      setDownloading(false);
    }
  };

  // Sentinel-style: minimal — no fill, just hover/active states
  const playBg = isPlaying
    ? "hsl(var(--primary) / 0.14)"
    : "hsl(var(--surface-4))";
  const playFg = isPlaying
    ? "hsl(var(--primary))"
    : !canPlay
    ? "hsl(var(--muted-foreground) / 0.5)"
    : "hsl(var(--foreground))";

  return (
    <div
      className="grid items-center gap-4 py-3 group"
      style={{
        gridTemplateColumns: "1fr 110px 110px 100px 104px",
        borderBottom: last ? "none" : "1px dashed hsl(var(--border))",
      }}
    >
      <div className="flex items-center gap-2 min-w-0">
        <span
          className="text-[13.5px] font-medium leading-snug truncate"
          style={{ color: "hsl(var(--foreground))" }}
          title={title}
        >
          {title}
        </span>
        <button
          onClick={handleCopy}
          title={copied ? "Copied!" : "Copy polished text"}
          className="w-6 h-6 rounded-md flex items-center justify-center flex-shrink-0 transition-all opacity-0 group-hover:opacity-100"
          style={{
            background: copied ? "hsl(var(--primary) / 0.18)" : "transparent",
            color:      copied ? "hsl(var(--primary))" : "hsl(var(--muted-foreground))",
          }}
        >
          {copied ? <Check size={11} strokeWidth={2.5} /> : <Copy size={11} />}
        </button>
        {live && !isPlaying && (
          <span
            className="text-[11px] flex-shrink-0"
            title="Most recent"
            style={{ color: "hsl(var(--primary))" }}
          >
            ●
          </span>
        )}
      </div>

      <div>
        <span
          className="inline-flex items-center gap-1.5 px-2.5 py-1 rounded-full text-[11px] font-semibold tabular-nums"
          style={{ background: chipBg, color: chipFg }}
        >
          {isPlaying && (
            <span
              className="inline-block w-1.5 h-1.5 rounded-full animate-pulse"
              style={{ background: "currentColor" }}
            />
          )}
          {chipText}
        </span>
      </div>

      <span className="text-[12.5px] tabular-nums"
            style={{ color: "hsl(var(--foreground))" }}>
        {relTime(rec.timestamp_ms)}
      </span>

      <span className="text-[12.5px] tabular-nums text-right font-semibold"
            style={{ color: "hsl(var(--foreground))" }}>
        {rec.word_count}
        <span className="text-[10px] ml-0.5"
              style={{ color: "hsl(var(--muted-foreground))" }}>
          · {model}
        </span>
      </span>

      <div className="flex justify-end gap-1.5">
        <button
          onClick={handleDownload}
          disabled={!canPlay || downloading}
          title={!canPlay ? "Audio not available" : downloading ? "Saving..." : "Download audio"}
          className="w-8 h-8 rounded-full flex items-center justify-center transition-all"
          style={{
            background: downloading ? "hsl(var(--primary) / 0.14)" : "hsl(var(--surface-4))",
            color:      !canPlay ? "hsl(var(--muted-foreground) / 0.5)" : "hsl(var(--foreground))",
            cursor:     canPlay && !downloading ? "pointer" : "not-allowed",
          }}
        >
          {downloading ? (
            <span className="inline-block w-2 h-2 rounded-full bg-current animate-pulse" />
          ) : (
            <Download size={11} />
          )}
        </button>
        <button
          onClick={onPlay}
          disabled={!canPlay}
          title={
            !canPlay   ? "Audio not available"
            : isPlaying ? "Pause"
            :             "Play recording"
          }
          className="w-8 h-8 rounded-full flex items-center justify-center transition-all"
          style={{
            background: playBg,
            color:      playFg,
            cursor:     canPlay ? "pointer" : "not-allowed",
          }}
        >
          {isPlaying ? (
            <Pause size={11} fill="currentColor" strokeWidth={0} />
          ) : (
            <Play size={11} fill="currentColor" strokeWidth={0} style={{ marginLeft: 1 }} />
          )}
        </button>
      </div>
    </div>
  );
}


/* ════════════════════════════════════════════════════════════════════════════
   NEW) TimelineCard — words dictated per day, 7d / 30d toggle.
   ════════════════════════════════════════════════════════════════════════════ */

export function TimelineCard({ recordings }: { recordings: Recording[] }) {
  const WEEKS = 16;
  const TOTAL_DAYS = WEEKS * 7;
  const now = new Date();
  now.setHours(0, 0, 0, 0);

  const todayDow = now.getDay();
  const startDate = new Date(now.getTime() - (TOTAL_DAYS - 1 + todayDow) * 86_400_000);
  startDate.setHours(0, 0, 0, 0);
  const totalDays = TOTAL_DAYS + todayDow;

  const dayMap = new Map<string, { words: number; count: number }>();
  for (let i = 0; i < totalDays; i++) {
    const d = new Date(startDate.getTime() + i * 86_400_000);
    dayMap.set(d.toISOString().slice(0, 10), { words: 0, count: 0 });
  }

  for (const r of recordings) {
    const d = new Date(r.timestamp_ms);
    d.setHours(0, 0, 0, 0);
    const key = d.toISOString().slice(0, 10);
    const entry = dayMap.get(key);
    if (entry) {
      entry.words += r.word_count;
      entry.count += 1;
    }
  }

  const allDays = Array.from(dayMap.entries()).map(([key, val]) => ({ key, ...val }));
  const maxWords = Math.max(...allDays.map((d) => d.words), 1);
  const totalWords = allDays.reduce((s, d) => s + d.words, 0);
  const activeDays = allDays.filter((d) => d.words > 0).length;
  const totalRecordings = allDays.reduce((s, d) => s + d.count, 0);

  const weeks: typeof allDays[] = [];
  for (let i = 0; i < allDays.length; i += 7) {
    weeks.push(allDays.slice(i, i + 7));
  }

  const cellColor = (words: number): string => {
    if (words === 0) return "hsl(var(--surface-4))";
    const ratio = words / maxWords;
    if (ratio < 0.25) return "hsl(140 50% 22%)";
    if (ratio < 0.50) return "hsl(140 55% 32%)";
    if (ratio < 0.75) return "hsl(140 60% 42%)";
    return "hsl(140 65% 52%)";
  };

  const monthLabels: { label: string; col: number }[] = [];
  let lastMonth = -1;
  weeks.forEach((week, wi) => {
    const firstDay = week[0];
    if (!firstDay) return;
    const m = new Date(firstDay.key).getMonth();
    if (m !== lastMonth) {
      monthLabels.push({
        label: new Date(firstDay.key).toLocaleDateString("en-US", { month: "short" }),
        col: wi,
      });
      lastMonth = m;
    }
  });

  const CELL = 11;
  const GAP = 3;
  const DOW_LABELS = ["", "Mon", "", "Wed", "", "Fri", ""];

  return (
    <div className="panel p-5">
      <div className="flex items-start justify-between gap-4 mb-4">
        <div>
          <h3
            className="text-[13px] font-bold tracking-tight"
            style={{ color: "hsl(var(--foreground))" }}
          >
            Activity
          </h3>
          <p
            className="text-[11.5px] mt-0.5"
            style={{ color: "hsl(var(--muted-foreground))" }}
          >
            {totalWords.toLocaleString()} words &middot; {totalRecordings} recording{totalRecordings !== 1 ? "s" : ""} &middot;{" "}
            {activeDays} active day{activeDays !== 1 ? "s" : ""}
          </p>
        </div>
      </div>

      <div className="flex overflow-x-auto">
        {/* Day-of-week labels */}
        <div className="flex flex-col flex-shrink-0 pr-2" style={{ gap: GAP, paddingTop: 14 + GAP }}>
          {DOW_LABELS.map((l, i) => (
            <div
              key={i}
              style={{
                height: CELL,
                fontSize: 9,
                lineHeight: `${CELL}px`,
                color: "hsl(var(--muted-foreground))",
                textAlign: "right",
              }}
            >
              {l}
            </div>
          ))}
        </div>

        {/* Week columns */}
        <div className="flex" style={{ gap: GAP }}>
          {weeks.map((week, wi) => (
            <div key={wi} className="flex flex-col" style={{ gap: GAP, width: CELL }}>
              {/* Month label row */}
              <div
                style={{
                  height: 14,
                  fontSize: 9,
                  lineHeight: "14px",
                  color: "hsl(var(--muted-foreground))",
                  whiteSpace: "nowrap",
                }}
              >
                {monthLabels.find((m) => m.col === wi)?.label ?? ""}
              </div>

              {/* Day cells */}
              {week.map((day) => {
                const isFuture = new Date(day.key).getTime() > now.getTime();
                return (
                  <div
                    key={day.key}
                    className="relative group"
                    style={{
                      width: CELL,
                      height: CELL,
                      borderRadius: 2,
                      background: isFuture ? "transparent" : cellColor(day.words),
                      opacity: isFuture ? 0 : 1,
                    }}
                  >
                    {day.words > 0 && (
                      <div
                        className="absolute bottom-full mb-1.5 left-1/2 -translate-x-1/2 px-2 py-1 rounded text-[10px] whitespace-nowrap pointer-events-none opacity-0 group-hover:opacity-100 transition-opacity z-20"
                        style={{
                          background: "hsl(var(--foreground))",
                          color: "hsl(var(--background))",
                          fontWeight: 600,
                          boxShadow: "0 4px 12px hsl(0 0% 0% / 0.18)",
                        }}
                      >
                        {day.words.toLocaleString()} words &middot;{" "}
                        {day.count} recording{day.count !== 1 ? "s" : ""} &middot;{" "}
                        {new Date(day.key).toLocaleDateString("en-US", {
                          month: "short",
                          day: "numeric",
                        })}
                      </div>
                    )}
                  </div>
                );
              })}
            </div>
          ))}
        </div>
      </div>

      <div className="flex items-center justify-end gap-1.5 mt-3">
        <span style={{ fontSize: 9, color: "hsl(var(--muted-foreground))" }}>Less</span>
        {[0, 0.15, 0.35, 0.65, 1].map((ratio, i) => (
          <div
            key={i}
            style={{
              width: CELL,
              height: CELL,
              borderRadius: 2,
              background: ratio === 0 ? "hsl(var(--surface-4))" : cellColor(ratio * maxWords),
            }}
          />
        ))}
        <span style={{ fontSize: 9, color: "hsl(var(--muted-foreground))" }}>More</span>
      </div>
    </div>
  );
}

/* ════════════════════════════════════════════════════════════════════════════
   NEW) AppBreakdownCard — fixes per app (target_app breakdown).
   ════════════════════════════════════════════════════════════════════════════ */

function appLabel(bundleId: string | null | undefined): string {
  if (!bundleId) return "Unknown";
  const MAP: Record<string, string> = {
    "com.apple.Safari":              "Safari",
    "com.apple.Notes":               "Notes",
    "com.apple.mail":                "Mail",
    "com.apple.Messages":            "Messages",
    "com.apple.TextEdit":            "TextEdit",
    "com.google.Chrome":             "Chrome",
    "org.chromium.Chromium":         "Chromium",
    "com.microsoft.VSCode":          "VS Code",
    "com.microsoft.Word":            "Word",
    "com.microsoft.Outlook":         "Outlook",
    "com.microsoft.PowerPoint":      "PowerPoint",
    "com.microsoft.Excel":           "Excel",
    "com.slack.Slack":               "Slack",
    "com.tinyspeck.slackmacgap":     "Slack",
    "com.notion.id":                 "Notion",
    "com.figma.Desktop":             "Figma",
    "company.thebrowser.Browser":    "Arc",
    "md.obsidian":                   "Obsidian",
    "com.apple.dt.Xcode":            "Xcode",
    "com.googlecode.iterm2":         "iTerm2",
    "com.apple.Terminal":            "Terminal",
    "com.spotify.client":            "Spotify",
    "com.linear.app":                "Linear",
    "com.discord.Discord":           "Discord",
    "com.zoom.us":                   "Zoom",
  };
  if (MAP[bundleId]) return MAP[bundleId];
  // Fallback: last segment of bundle ID, title-cased
  const parts = bundleId.split(".");
  const last  = parts[parts.length - 1];
  return last.charAt(0).toUpperCase() + last.slice(1);
}

// Distinct hues for the top apps so they're visually distinguishable
const APP_COLORS = [
  "hsl(var(--primary))",
  "hsl(210 80% 55%)",
  "hsl(270 70% 60%)",
  "hsl(38  85% 55%)",
  "hsl(150 60% 45%)",
  "hsl(10  75% 55%)",
];

export function AppBreakdownCard({ recordings }: { recordings: Recording[] }) {
  const byApp = new Map<string, { words: number; sessions: number }>();
  for (const r of recordings) {
    const key      = r.target_app ?? "__unknown__";
    const existing = byApp.get(key) ?? { words: 0, sessions: 0 };
    byApp.set(key, {
      words:    existing.words    + r.word_count,
      sessions: existing.sessions + 1,
    });
  }

  const entries = Array.from(byApp.entries())
    .map(([bundleId, stats]) => ({
      label:    appLabel(bundleId === "__unknown__" ? null : bundleId),
      bundleId,
      ...stats,
    }))
    .sort((a, b) => b.words - a.words)
    .slice(0, 6);

  const maxWords = Math.max(...entries.map((e) => e.words), 1);

  if (entries.length === 0) {
    return (
      <div className="panel p-5 flex items-center justify-center" style={{ minHeight: 120 }}>
        <p className="text-[12px]" style={{ color: "hsl(var(--muted-foreground))" }}>
          No recordings yet
        </p>
      </div>
    );
  }

  return (
    <div className="panel p-5">
      <div className="mb-4">
        <h3 className="text-[13px] font-bold tracking-tight"
            style={{ color: "hsl(var(--foreground))" }}>
          Fixes by app
        </h3>
        <p className="text-[11.5px] mt-0.5" style={{ color: "hsl(var(--muted-foreground))" }}>
          Which apps you dictate into most
        </p>
      </div>

      <div className="space-y-3.5">
        {entries.map((entry, idx) => {
          const pct   = (entry.words / maxWords) * 100;
          const color = APP_COLORS[idx % APP_COLORS.length];
          return (
            <div key={entry.bundleId}>
              <div className="flex items-baseline justify-between mb-1 gap-2">
                <span
                  className="text-[12px] font-semibold truncate"
                  style={{ color: "hsl(var(--foreground))" }}
                >
                  {entry.label}
                </span>
                <span
                  className="text-[11px] tabular-nums flex-shrink-0"
                  style={{ color: "hsl(var(--muted-foreground))" }}
                >
                  {entry.words.toLocaleString()} words
                  <span style={{ opacity: 0.5 }}> · </span>
                  {entry.sessions} {entry.sessions === 1 ? "session" : "sessions"}
                </span>
              </div>
              <div
                className="w-full rounded-full overflow-hidden"
                style={{ height: 5, background: "hsl(var(--surface-4))" }}
              >
                <div
                  style={{
                    width:        `${pct}%`,
                    height:       "100%",
                    background:   color,
                    borderRadius: "9999px",
                    transition:   "width 0.4s ease",
                  }}
                />
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
