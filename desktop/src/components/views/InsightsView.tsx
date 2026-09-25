import { startTransition, useEffect, useState } from "react";
import {
  Clock3,
  Gauge,
  Monitor,
  Sparkles,
  Target,
  TrendingUp,
} from "lucide-react";
import { AppIcon, appDisplayName, useAppIdentity } from "@/components/AppIcon";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Skeleton } from "@/components/ui/skeleton";
import {
  getHistoryCacheSnapshot,
  refreshHistoryCache,
  subscribeHistoryCache,
} from "@/lib/historyUiCache";
import {
  currentInsightStreak,
  buildHeatmapDays,
  longestInsightStreakInWindow,
} from "@/lib/insights";
import type { Recording } from "@/types";

type ActivityRange = "30d" | "90d" | "all";

const DAY_MS = 86_400_000;
const TYPING_WPM = 40;
const CONSERVATIVE_SPEAKING_WPM = 120;

function rangeCutoff(range: ActivityRange): number {
  if (range === "all") return 0;
  return Date.now() - (range === "30d" ? 30 : 90) * DAY_MS;
}

function formatDuration(minutes: number): string {
  if (minutes < 60) return `${Math.round(minutes)}m`;
  const hours = Math.floor(minutes / 60);
  const remaining = Math.round(minutes % 60);
  return remaining ? `${hours}h ${remaining}m` : `${hours}h`;
}

function formatCount(value: number): string {
  return new Intl.NumberFormat("en-US", { notation: value >= 10_000 ? "compact" : "standard", maximumFractionDigits: 1 }).format(value);
}

export function InsightsView() {
  const [range, setRange] = useState<ActivityRange>("30d");
  const [historySnapshot, setHistorySnapshot] = useState(() => getHistoryCacheSnapshot());

  useEffect(() => {
    const sync = () => {
      startTransition(() => {
        setHistorySnapshot(getHistoryCacheSnapshot());
      });
    };
    sync();
    const unsubscribeHistory = subscribeHistoryCache(sync);
    void refreshHistoryCache({ limit: 2_000 });
    return () => {
      unsubscribeHistory();
    };
  }, []);

  const recordings = historySnapshot.recordings ?? [];
  const loading = historySnapshot.recordings === undefined;

  const visible = recordings.filter((recording) => recording.timestamp_ms >= rangeCutoff(range));
  const words = visible.reduce((sum, recording) => sum + recording.word_count, 0);
  const pacedRecordings = visible.filter((recording) => recording.recording_seconds > 0);
  const pacedWords = pacedRecordings.reduce((sum, recording) => sum + recording.word_count, 0);
  const audioSeconds = pacedRecordings.reduce((sum, recording) => sum + recording.recording_seconds, 0);
  const pace = audioSeconds > 0 ? Math.round((pacedWords / audioSeconds) * 60) : 0;
  const savedMinutes = Math.max(0, words / TYPING_WPM - words / CONSERVATIVE_SPEAKING_WPM);
  const currentStreak = currentInsightStreak(recordings);
  const longestStreak = longestInsightStreakInWindow(recordings);

  const appMap = new Map<string, { words: number; sessions: number }>();
  for (const recording of visible) {
    const key = recording.target_app?.trim();
    if (!key) continue;
    const existing = appMap.get(key) ?? { words: 0, sessions: 0 };
    existing.words += recording.word_count;
    existing.sessions += 1;
    appMap.set(key, existing);
  }
  const appWords = [...appMap.values()].reduce((sum, app) => sum + app.words, 0);
  const topApps = [...appMap.entries()]
    .map(([key, value]) => ({ key, ...value }))
    .sort((a, b) => b.words - a.words)
    .slice(0, 5);

  return (
    <ScrollArea className="h-full">
      <div className="insights-page">
        <header className="insights-header">
          <div>
            <h1>Insights</h1>
            <p className="insights-subtitle">Your momentum, on this Mac</p>
          </div>
        </header>

        {loading ? <InsightsSkeleton /> : (
          <ActivityTab
            range={range}
            setRange={setRange}
            words={words}
            pace={pace}
            savedMinutes={savedMinutes}
            recordings={recordings}
            visible={visible}
            currentStreak={currentStreak}
            longestStreak={longestStreak}
            topApps={topApps}
            appWords={appWords}
          />
        )}
      </div>
    </ScrollArea>
  );
}

function ActivityTab({
  range,
  setRange,
  words,
  pace,
  savedMinutes,
  recordings,
  visible,
  currentStreak,
  longestStreak,
  topApps,
  appWords,
}: {
  range: ActivityRange;
  setRange: (range: ActivityRange) => void;
  words: number;
  pace: number;
  savedMinutes: number;
  recordings: Recording[];
  visible: Recording[];
  currentStreak: number;
  longestStreak: number;
  topApps: Array<{ key: string; words: number; sessions: number }>;
  appWords: number;
}) {
  return (
    <div className="insights-reveal" role="tabpanel">
      <div className="insights-toolbar">
        <p>{visible.length.toLocaleString()} dictation{visible.length === 1 ? "" : "s"} in this period</p>
        <div className="insights-range" aria-label="Activity range">
          {(["30d", "90d", "all"] as const).map((option) => (
            <button key={option} type="button" aria-pressed={range === option} onClick={() => setRange(option)}>
              {option === "all" ? "All time" : option.toUpperCase()}
            </button>
          ))}
        </div>
      </div>

      <section className="insights-metric-grid" aria-label="Usage summary">
        <MetricCard icon={<TrendingUp size={15} />} label="Words dictated" value={formatCount(words)} detail={range === "all" ? "Across your available history" : `In the last ${range === "30d" ? 30 : 90} days`} />
        <MetricCard icon={<Clock3 size={15} />} label="Time reclaimed" value={formatDuration(savedMinutes)} detail="Estimate vs typing at 40 WPM" />
        <MetricCard icon={<Gauge size={15} />} label="Speaking pace" value={pace ? `${pace}` : "—"} suffix={pace ? "WPM" : undefined} detail="Weighted by recorded speech" />
      </section>

      <section className="insights-two-column">
        <ActivityHeatmap recordings={recordings} currentStreak={currentStreak} longestStreak={longestStreak} />
        <div className="insights-card insights-app-card">
          <div className="insights-card-heading"><div><p className="insights-kicker">Distribution</p><h2>Where your words go</h2></div><Monitor size={17} /></div>
          {topApps.length ? (
            <div className="insights-app-list">
              {topApps.map((app) => <AppUsageRow key={app.key} app={app} total={appWords} />)}
            </div>
          ) : <EmptyInsight text="App usage will appear after your next dictation." />}
        </div>
      </section>

    </div>
  );
}

function MetricCard({ icon, label, value, suffix, detail }: { icon: React.ReactNode; label: string; value: string; suffix?: string; detail: string }) {
  return <div className="insights-metric-card"><div className="insights-metric-label">{icon}<span>{label}</span></div><div className="insights-metric-value">{value}{suffix && <small>{suffix}</small>}</div><p>{detail}</p></div>;
}

function ActivityHeatmap({ recordings, currentStreak, longestStreak }: { recordings: Recording[]; currentStreak: number; longestStreak: number }) {
  const days = buildHeatmapDays(recordings, 16);
  const maxWords = Math.max(1, ...days.map((day) => day.words));

  return (
    <div className="insights-card insights-heat-card">
      <div className="insights-card-heading"><div><p className="insights-kicker">Last 16 weeks</p><h2>Consistency</h2></div><Target size={17} /></div>
      <div className="insights-streak-line"><strong>{currentStreak} day streak</strong><span>Longest: {longestStreak} days</span></div>
      <div className="insights-heatmap" aria-label="Daily dictated words over the last 16 weeks">
        {days.map((day) => {
          if (day.isFuture) {
            return <span key={day.key} className="heat-level-future" title="Future" aria-hidden />;
          }
          const ratio = day.words / maxWords;
          const level = day.words === 0 ? 0 : ratio < 0.25 ? 1 : ratio < 0.5 ? 2 : ratio < 0.75 ? 3 : 4;
          return <span key={day.key} className={`heat-level-${level}`} title={`${day.date.toLocaleDateString()}: ${day.words.toLocaleString()} words`} />;
        })}
      </div>
      <div className="insights-heat-legend"><span>Less</span>{[0, 1, 2, 3, 4].map((level) => <i key={level} className={`heat-level-${level}`} />)}<span>More</span></div>
    </div>
  );
}

function AppUsageRow({ app, total }: { app: { key: string; words: number; sessions: number }; total: number }) {
  const identity = useAppIdentity(app.key);
  const percentage = total ? Math.round((app.words / total) * 100) : 0;
  return (
    <div className="insights-app-row">
      <AppIcon appKey={app.key} size={34} radius={9} />
      <div className="insights-app-main"><div><strong>{appDisplayName(app.key, identity)}</strong><span>{identity?.category || `${app.sessions} dictations`}</span></div><div className="insights-app-track"><span style={{ width: `${percentage}%` }} /></div></div>
      <div className="insights-app-value"><strong>{percentage}%</strong><span>{app.words.toLocaleString()} words</span></div>
    </div>
  );
}

function EmptyInsight({ text }: { text: string }) {
  return <div className="insights-empty"><Sparkles size={16} /><p>{text}</p></div>;
}

function InsightsSkeleton() {
  return (
    <div className="insights-reveal" aria-label="Loading insights">
      <div className="insights-toolbar">
        <Skeleton className="h-3 w-40" />
        <Skeleton className="h-7 w-32" style={{ borderRadius: 10 }} />
      </div>
      <section className="insights-metric-grid">
        {[0, 1, 2].map((i) => (
          <div key={i} className="insights-metric-card">
            <Skeleton className="h-2.5 w-24" />
            <Skeleton className="mt-4 h-7 w-20" />
            <Skeleton className="mt-3 h-2.5 w-32" />
          </div>
        ))}
      </section>
      <section className="insights-two-column">
        <div className="insights-card">
          <Skeleton className="h-2.5 w-20" />
          <Skeleton className="mt-2 h-4 w-28" />
          <Skeleton className="mt-4 w-full" style={{ aspectRatio: "16 / 7" }} />
        </div>
        <div className="insights-card">
          <Skeleton className="h-2.5 w-20" />
          <Skeleton className="mt-2 h-4 w-32" />
          <div className="mt-4 space-y-3">
            {[0, 1, 2, 3, 4].map((i) => (
              <div key={i} className="flex items-center gap-3">
                <Skeleton className="h-8 w-8 rounded-lg" />
                <Skeleton className="h-3 flex-1" style={{ maxWidth: `${70 - i * 8}%` }} />
                <Skeleton className="h-3 w-8" />
              </div>
            ))}
          </div>
        </div>
      </section>
    </div>
  );
}
