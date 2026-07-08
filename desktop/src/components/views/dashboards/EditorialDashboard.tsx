import { useEffect, useMemo, useState } from "react";
import { ScrollArea } from "@/components/ui/scroll-area";
import { listHistory, downloadRecordingAudio, getRecordingAudioBytes, openExternal, getAppIcon } from "@/lib/invoke";
import { cn } from "@/lib/utils";
import { Copy, Download, Check, Play, Square, BookOpen, ExternalLink } from "lucide-react";
import { AppIcon, appDisplayName, useAppIdentity } from "@/components/AppIcon";
import type { AppSnapshot, Recording } from "@/types";

interface Props {
  snapshot:      AppSnapshot | null;
}

const GUIDE_URL = "https://airnote.emiactech.com/guide";

// Persist that the user has opened the guide so the dashboard prompt is shown
// only until they've read it once. Mirrors the localStorage pattern used by
// onboarding/enterprise state.
const GUIDE_SEEN_KEY = "said:guide-opened";

function guideWasOpened(): boolean {
  try {
    return localStorage.getItem(GUIDE_SEEN_KEY) === "1";
  } catch {
    return false;
  }
}

function markGuideOpened(): void {
  try {
    localStorage.setItem(GUIDE_SEEN_KEY, "1");
  } catch {
    /* localStorage unavailable — prompt simply reappears next session */
  }
}

/**
 * Editorial dashboard — pattern B from the layout proposals.
 *
 * Single-column, magazine-style. Opens with a personalised headline,
 * then sectioned blocks: At a glance / Activity / Latest.
 * Calm reading mode rather than analytics console.
 */
export function EditorialDashboard({ snapshot }: Props) {
  const [recordings, setRecordings] = useState<Recording[]>([]);
  const [guideOpened, setGuideOpened] = useState(guideWasOpened);
  const history = snapshot?.history ?? [];

  useEffect(() => {
    let alive = true;
    listHistory(300).then((r) => { if (alive) setRecordings(r); });
    return () => { alive = false; };
  }, [history.length]);

  // ── Derived: today's words + time saved ─────────────────────────────────
  const today = useMemo(() => {
    const now = new Date();
    const startMs = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
    return recordings.filter((r) => r.timestamp_ms >= startMs);
  }, [recordings]);

  const wordsToday  = today.reduce((s, r) => s + r.word_count, 0);
  // Estimate time saved vs. typing at 40 WPM.
  // recording_seconds includes silence/hold time, so we use the polished
  // word count at an assumed 120 WPM speaking rate (conservative; natural
  // speech is 130-170 WPM) to estimate actual dictation effort.
  const typingMinutesToday = wordsToday > 0 ? wordsToday / 40 : 0;
  const dictMinutesToday = wordsToday > 0 ? wordsToday / 120 : 0;
  const minutesSaved = Math.max(0, Math.round(typingMinutesToday - dictMinutesToday));

  const editsLearned = useMemo(
    () => recordings.reduce((s, r) => s + (r.edit_count ?? 0), 0),
    [recordings],
  );

  // ── Bars: last 14 days, height ∝ word count ─────────────────────────────
  const bars = useMemo(() => buildLast14Days(recordings), [recordings]);
  const maxBar = Math.max(1, ...bars.map((b) => b.words));

  // ── Top apps: rank the apps you dictate in most (by count) ───────────────
  const topApps = useMemo(() => {
    const counts = new Map<string, number>();
    for (const r of recordings) {
      const app = r.target_app?.trim();
      if (!app) continue;
      counts.set(app, (counts.get(app) ?? 0) + 1);
    }
    return [...counts.entries()]
      .sort((a, b) => b[1] - a[1])
      .slice(0, 4)
      .map(([key, count]) => ({ key, count }));
  }, [recordings]);

  const [appIcons, setAppIcons] = useState<Record<string, string | null>>({});
  const topKeys = topApps.map((a) => a.key).join(",");
  useEffect(() => {
    let alive = true;
    Promise.all(
      topApps.map(async (a) => [a.key, await getAppIcon(a.key)] as const),
    ).then((pairs) => {
      if (alive) setAppIcons(Object.fromEntries(pairs));
    });
    return () => { alive = false; };
  }, [topKeys, topApps]);

  // ── Today's recordings ──────────────────────────────────────────────────
  const latest = today;

  const dateLabel = useMemo(() => formatDate(new Date()), []);

  return (
    <ScrollArea className="h-full">
      <div className="mx-auto" style={{ maxWidth: "min(720px, 100%)", padding: "24px 28px 40px" }}>

        {/* Hero — personalised headline */}
        <div className="mb-7">
          <p
            className="text-[11px] font-semibold uppercase tracking-[0.16em] mb-2.5"
            style={{ color: "hsl(var(--primary))" }}
          >
            Today · {dateLabel}
          </p>
          <h1
            className="m-0 leading-tight"
            style={{
              fontSize: 28,
              fontWeight: 600,
              letterSpacing: "-0.025em",
              color: "hsl(var(--foreground))",
            }}
          >
            {wordsToday === 0
              ? "Ready when you are."
              : `You dictated ${wordsToday.toLocaleString()} ${wordsToday === 1 ? "word" : "words"} today.`}
          </h1>
          <p
            className="mt-2.5 mb-0 leading-relaxed"
            style={{ fontSize: 13.5, color: "hsl(var(--muted-foreground))", maxWidth: "min(540px, 100%)" }}
          >
            {wordsToday === 0
              ? "Hold your hotkey and speak — AirNote will type polished text into the focused app. Your daily summary appears here once you've started."
              : (
                <>
                  That's roughly{" "}
                  <em className="not-italic font-semibold" style={{ color: "hsl(var(--primary))" }}>
                    {minutesSaved} {minutesSaved === 1 ? "minute" : "minutes"}
                  </em>{" "}
                  saved vs. typing, at an average of {snapshot?.avg_wpm ?? 0} WPM.
                </>
              )}
          </p>
        </div>

        {!guideOpened && (
          <GuidePrompt
            onOpen={() => {
              markGuideOpened();
              setGuideOpened(true);
            }}
          />
        )}

        {/* At a glance — 3-column row, hairline separators */}
        <Section label="At a glance">
          <div
            className="grid"
            style={{
              gridTemplateColumns: "repeat(auto-fit, minmax(140px, 1fr))",
              padding: "14px 0",
              gap: "12px 0",
              borderTop: "1px solid hsl(var(--border))",
              borderBottom: "1px solid hsl(var(--border))",
            }}
          >
            {topApps.length > 0 && (
              <div style={{ padding: "0 14px", minWidth: 0 }}>
                <div
                  className="text-[10px] uppercase"
                  style={{ color: "hsl(var(--muted-foreground))", letterSpacing: "0.12em" }}
                >
                  Top apps
                </div>
                <div className="mt-2">
                  <TopAppsRow apps={topApps} icons={appIcons} />
                </div>
              </div>
            )}
            <Glance label="Avg pace" value={`${snapshot?.avg_wpm ?? 0}`} unit="wpm" border={topApps.length > 0} />
            <Glance label="Edits learned" value={`${editsLearned}`} unit="" border />
          </div>
        </Section>

        {/* Activity — last 14 days, bar chart */}
        <Section label="Activity — last 14 days">
          <div style={{ display: "flex", alignItems: "flex-end", gap: 4, height: 64, paddingTop: 6, overflow: "hidden", minWidth: 0 }}>
            {bars.map((b, i) => {
              const h = b.words === 0 ? 8 : Math.max(8, Math.round((b.words / maxBar) * 64));
              const isAccent = i % 2 === 1;
              return (
                <div
                  key={b.dayKey}
                  title={`${b.label} · ${b.words} words`}
                  style={{
                    flex: 1,
                    height: h,
                    borderRadius: 2,
                    background: b.words === 0
                      ? "hsl(var(--surface-4) / 0.52)"
                      : isAccent
                        ? "hsl(var(--primary) / 0.45)"
                        : "hsl(var(--primary) / 0.20)",
                  }}
                />
              );
            })}
          </div>
          <div
            className="flex justify-between mt-2"
            style={{ fontSize: 10.5, color: "hsl(var(--muted-foreground))", fontFamily: "ui-monospace, SF Mono, monospace" }}
          >
            <span>{bars[0]?.short}</span>
            <span>{bars[Math.floor(bars.length / 2)]?.short}</span>
            <span>{bars[bars.length - 1]?.short}</span>
          </div>
        </Section>

        {/* Today's history — all recordings from last 24h */}
        <Section label={`Today — ${latest.length} recording${latest.length !== 1 ? "s" : ""}`}>
          {latest.length === 0 ? (
            <p className="text-[13px] py-4" style={{ color: "hsl(var(--muted-foreground))" }}>
              Your first dictation will land here.
            </p>
          ) : (
            <div>
              {latest.map((r) => (
                <HistoryRow key={r.id} recording={r} />
              ))}
            </div>
          )}
        </Section>
      </div>
    </ScrollArea>
  );
}

// ── Tiny presentational primitives (file-local) ───────────────────────────────

function GuidePrompt({ onOpen }: { onOpen: () => void }) {
  return (
    <button
      type="button"
      onClick={() => {
        onOpen();
        void openExternal(GUIDE_URL);
      }}
      className="group w-full mb-7 text-left"
      style={{
        display: "grid",
        gridTemplateColumns: "36px minmax(0, 1fr) auto",
        alignItems: "center",
        gap: 12,
        padding: "12px 0",
        borderTop: "1px solid hsl(var(--border))",
        borderBottom: "1px solid hsl(var(--border))",
      }}
      title="Open AirNote guide and shortcuts"
    >
      <span
        className="grid place-items-center"
        style={{
          width: 36,
          height: 36,
          borderRadius: 8,
          background: "hsl(var(--primary) / 0.10)",
          color: "hsl(var(--primary))",
        }}
      >
        <BookOpen size={18} />
      </span>
      <span className="min-w-0">
        <span
          className="block"
          style={{
            fontSize: 13,
            fontWeight: 650,
            color: "hsl(var(--foreground))",
          }}
        >
          New here? Open the guide.
        </span>
        <span
          className="block truncate"
          style={{
            marginTop: 2,
            fontSize: 12,
            color: "hsl(var(--muted-foreground))",
          }}
        >
          Shortcuts, dictation, retry, polish mode, and status pill placement.
        </span>
      </span>
      <span
        className="inline-flex items-center gap-1 text-[12px] font-semibold"
        style={{ color: "hsl(var(--primary))" }}
      >
        Open
        <ExternalLink size={13} />
      </span>
    </button>
  );
}

/**
 * Top apps — the apps you dictate in most, as a small, subtle fanned stack that
 * sits inline beside the hero headline: #1 in front, each next one smaller and
 * tucked behind to the right. Icons only (rank + count on hover).
 */
function TopAppsRow({
  apps,
  icons,
}: {
  apps: { key: string; count: number }[];
  icons: Record<string, string | null>;
}) {
  const SIZES = [34, 27, 22, 19];
  const items = apps.slice(0, 4);

  const lefts: number[] = [];
  let cursor = 0;
  items.forEach((_, i) => {
    lefts.push(cursor);
    cursor += Math.round(SIZES[i] * 0.85);
  });

  const height = SIZES[0];
  const lastIdx = items.length - 1;
  const width = (lefts[lastIdx] ?? 0) + (SIZES[lastIdx] ?? SIZES[0]);

  return (
    <div className="flex-shrink-0" style={{ position: "relative", height, width, minWidth: width }}>
      {items.map((app, i) => {
        const size = SIZES[i];
        const icon = icons[app.key];
        return (
          <div
            key={app.key}
            title={`#${i + 1} · ${app.count} dictation${app.count === 1 ? "" : "s"}`}
            style={{
              position: "absolute",
              left: lefts[i],
              top: (height - size) / 2,
              width: size,
              height: size,
              zIndex: 40 - i * 10,
              borderRadius: Math.round(size * 0.28),
              overflow: "hidden",
              border: "1.5px solid hsl(var(--background))",
              boxShadow: "0 1px 4px hsl(0 0% 0% / 0.3)",
              background: "hsl(var(--surface-3))",
              opacity: i === 0 ? 1 : 0.9,
            }}
          >
            {icon ? (
              <img
                src={icon}
                alt=""
                style={{ width: "100%", height: "100%", objectFit: "cover" }}
              />
            ) : (
              <div style={{ width: "100%", height: "100%", background: "hsl(var(--surface-4))" }} />
            )}
          </div>
        );
      })}
    </div>
  );
}

function Section({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <section className="mb-7">
      <h2
        className="m-0 mb-3"
        style={{
          fontSize: 11,
          fontWeight: 700,
          letterSpacing: "0.14em",
          textTransform: "uppercase",
          color: "hsl(var(--muted-foreground))",
        }}
      >
        {label}
      </h2>
      {children}
    </section>
  );
}

function Glance({ label, value, unit, border }: { label: string; value: string; unit: string; border?: boolean }) {
  return (
    <div style={{ padding: "0 14px", borderLeft: border ? "1px solid hsl(var(--border))" : "none", minWidth: 0 }}>
      <div className="text-[10px] uppercase" style={{ color: "hsl(var(--muted-foreground))", letterSpacing: "0.12em" }}>
        {label}
      </div>
      <div className="mt-1.5" style={{ fontSize: 24, fontWeight: 600, letterSpacing: "-0.02em", color: "hsl(var(--foreground))" }}>
        {value}
        {unit && (
          <span className="ml-1.5" style={{ fontSize: 13, color: "hsl(var(--muted-foreground))", fontWeight: 500 }}>
            {unit}
          </span>
        )}
      </div>
    </div>
  );
}

// ── Helpers ──────────────────────────────────────────────────────────────────

function formatDate(d: Date): string {
  return d.toLocaleDateString("en-US", { day: "numeric", month: "short" });
}

function relativeTime(ms: number): string {
  const sec = Math.max(1, Math.floor((Date.now() - ms) / 1000));
  if (sec < 60) return `${sec}s ago`;
  const m = Math.floor(sec / 60);
  if (m < 60) return `${m}m ago`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ago`;
  const d = Math.floor(h / 24);
  return `${d}d ago`;
}

function buildLast14Days(recordings: Recording[]): { dayKey: string; label: string; short: string; words: number }[] {
  const out: { dayKey: string; label: string; short: string; words: number }[] = [];
  const now = new Date();
  for (let i = 13; i >= 0; i--) {
    const d = new Date(now.getFullYear(), now.getMonth(), now.getDate() - i);
    const dayKey = `${d.getFullYear()}-${d.getMonth()}-${d.getDate()}`;
    const next = new Date(d); next.setDate(d.getDate() + 1);
    const words = recordings
      .filter((r) => r.timestamp_ms >= d.getTime() && r.timestamp_ms < next.getTime())
      .reduce((s, r) => s + r.word_count, 0);
    out.push({
      dayKey,
      label: d.toLocaleDateString("en-US", { weekday: "short", day: "numeric", month: "short" }),
      short: d.toLocaleDateString("en-US", { day: "numeric" }),
      words,
    });
  }
  return out;
}

let _activeAudio: HTMLAudioElement | null = null;

function HistoryRow({ recording: r }: { recording: Recording }) {
  const [copied, setCopied] = useState(false);
  const [downloading, setDownloading] = useState(false);
  const [playing, setPlaying] = useState(false);
  const [hasAudio, setHasAudio] = useState(Boolean(r.audio_id));
  const appIdentity = useAppIdentity(r.target_app);
  const appName = appDisplayName(r.target_app, appIdentity);

  useEffect(() => {
    let alive = true;
    if (r.audio_id) {
      setHasAudio(true);
      return;
    }
    void getRecordingAudioBytes(r.id).then((bytes) => {
      if (alive && bytes && bytes.length > 0) setHasAudio(true);
    });
    return () => {
      alive = false;
    };
  }, [r.id, r.audio_id]);

  const handleCopy = async () => {
    try {
      await navigator.clipboard.writeText(r.polished || r.transcript);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch { /* clipboard not available */ }
  };

  const handlePlay = async () => {
    if (!hasAudio) return;
    if (playing && _activeAudio) {
      _activeAudio.pause();
      _activeAudio = null;
      setPlaying(false);
      return;
    }
    try {
      const bytes = await getRecordingAudioBytes(r.id);
      if (!bytes) return;
      const blob = new Blob([new Uint8Array(bytes)], { type: "audio/wav" });
      const blobUrl = URL.createObjectURL(blob);
      if (_activeAudio) { _activeAudio.pause(); _activeAudio = null; }
      const audio = new Audio(blobUrl);
      _activeAudio = audio;
      setPlaying(true);
      audio.onended = () => { setPlaying(false); _activeAudio = null; URL.revokeObjectURL(blobUrl); };
      audio.onerror = () => { setPlaying(false); _activeAudio = null; URL.revokeObjectURL(blobUrl); };
      audio.play();
    } catch { setPlaying(false); }
  };

  const handleDownload = async () => {
    if (!hasAudio) return;
    setDownloading(true);
    try {
      const filename = `airnote-${new Date(r.timestamp_ms).toISOString().slice(0, 10)}-${r.id.slice(0, 8)}.wav`;
      await downloadRecordingAudio(r.id, filename);
    } catch { /* download failed */ }
    setDownloading(false);
  };

  const keepActionsVisible = playing || copied || downloading;

  return (
    <div
      className="group/row flex items-start gap-3 py-3"
      style={{ borderBottom: "1px solid hsl(var(--border))" }}
    >
      <AppIcon appKey={r.target_app} label={appName} size={26} radius={7} fallbackSize={14} style={{ marginTop: 2 }} />
      <div className="flex-1 min-w-0">
        <div
          className="text-[13.5px] leading-snug truncate"
          style={{ color: "hsl(var(--foreground))" }}
          title={r.polished || r.transcript || undefined}
        >
          {r.polished || r.transcript}
        </div>
        <div className="flex items-center gap-2 mt-1">
          <span
            className="text-[11px]"
            style={{ color: "hsl(var(--muted-foreground))", fontFamily: "ui-monospace, SF Mono, monospace" }}
          >
            {relativeTime(r.timestamp_ms)}
          </span>
          {r.word_count > 0 && (
            <span className="text-[10px]" style={{ color: "hsl(var(--muted-foreground) / 0.6)" }}>
              {r.word_count} words
            </span>
          )}
          {r.target_app && (
            <span className="text-[10px] truncate" style={{ color: "hsl(var(--muted-foreground) / 0.62)", maxWidth: 140 }}>
              {appName}
            </span>
          )}
        </div>
      </div>
      <div
        className={cn(
          "flex items-center gap-1 pt-0.5 transition-opacity duration-150",
          "opacity-0 pointer-events-none",
          "group-hover/row:opacity-100 group-hover/row:pointer-events-auto",
          "group-focus-within/row:opacity-100 group-focus-within/row:pointer-events-auto",
          keepActionsVisible && "opacity-100 pointer-events-auto",
        )}
      >
        {hasAudio && (
          <button
            onClick={handlePlay}
            className="p-1.5 rounded-md hover:bg-[hsl(var(--surface-hover))] transition-colors"
            title={playing ? "Stop" : "Play audio"}
          >
            {playing
              ? <Square size={13} style={{ color: "hsl(var(--primary))" }} />
              : <Play size={14} style={{ color: "hsl(var(--muted-foreground))" }} />
            }
          </button>
        )}
        <button
          onClick={handleCopy}
          className="p-1.5 rounded-md hover:bg-[hsl(var(--surface-hover))] transition-colors"
          title="Copy to clipboard"
        >
          {copied
            ? <Check size={14} style={{ color: "hsl(var(--primary))" }} />
            : <Copy size={14} style={{ color: "hsl(var(--muted-foreground))" }} />
          }
        </button>
        {hasAudio && (
          <button
            onClick={handleDownload}
            disabled={downloading}
            className="p-1.5 rounded-md hover:bg-[hsl(var(--surface-hover))] transition-colors"
            title="Download WAV"
          >
            <Download
              size={14}
              style={{ color: downloading ? "hsl(var(--primary))" : "hsl(var(--muted-foreground))" }}
            />
          </button>
        )}
      </div>
    </div>
  );
}
