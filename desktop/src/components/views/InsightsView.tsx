import { startTransition, useEffect, useState } from "react";
import {
  ArrowUpRight,
  BookOpen,
  Check,
  Clock3,
  Gauge,
  Languages,
  Monitor,
  Sparkles,
  Target,
  TrendingUp,
} from "lucide-react";
import { AppIcon, appDisplayName, useAppIdentity } from "@/components/AppIcon";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Skeleton } from "@/components/ui/skeleton";
import {
  type VocabAlias,
  type VocabRow,
} from "@/lib/invoke";
import {
  getHistoryCacheSnapshot,
  refreshHistoryCache,
  subscribeHistoryCache,
} from "@/lib/historyUiCache";
import {
  getVocabularyCacheSnapshot,
  refreshVocabularyCache,
  subscribeVocabularyCache,
} from "@/lib/vocabularyUiCache";
import {
  currentInsightStreak,
  longestInsightStreak,
} from "@/lib/insights";
import type { Recording } from "@/types";

type InsightTab = "activity" | "learning";
type ActivityRange = "30d" | "90d" | "all";

const DAY_MS = 86_400_000;
const TYPING_WPM = 40;
const CONSERVATIVE_SPEAKING_WPM = 120;

function rangeCutoff(range: ActivityRange): number {
  if (range === "all") return 0;
  return Date.now() - (range === "30d" ? 30 : 90) * DAY_MS;
}

function localDayKey(timestampMs: number): string {
  const date = new Date(timestampMs);
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
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
  const [tab, setTab] = useState<InsightTab>("activity");
  const [range, setRange] = useState<ActivityRange>("30d");
  const [historySnapshot, setHistorySnapshot] = useState(() => getHistoryCacheSnapshot());
  const [vocabularySnapshot, setVocabularySnapshot] = useState(() => getVocabularyCacheSnapshot());

  useEffect(() => {
    const sync = () => {
      startTransition(() => {
        setHistorySnapshot(getHistoryCacheSnapshot());
        setVocabularySnapshot(getVocabularyCacheSnapshot());
      });
    };
    sync();
    const unsubscribeHistory = subscribeHistoryCache(sync);
    const unsubscribeVocabulary = subscribeVocabularyCache(sync);
    void refreshHistoryCache({ limit: 2_000 });
    void refreshVocabularyCache();
    return () => {
      unsubscribeHistory();
      unsubscribeVocabulary();
    };
  }, []);

  const recordings = historySnapshot.recordings ?? [];
  const vocabulary = vocabularySnapshot.terms ?? [];
  const aliases = vocabularySnapshot.aliases ?? [];
  const loading = historySnapshot.recordings === undefined
    || vocabularySnapshot.terms === undefined
    || vocabularySnapshot.aliases === undefined;

  const visible = recordings.filter((recording) => recording.timestamp_ms >= rangeCutoff(range));
  const words = visible.reduce((sum, recording) => sum + recording.word_count, 0);
  const pacedRecordings = visible.filter((recording) => recording.recording_seconds > 0);
  const pacedWords = pacedRecordings.reduce((sum, recording) => sum + recording.word_count, 0);
  const audioSeconds = pacedRecordings.reduce((sum, recording) => sum + recording.recording_seconds, 0);
  const pace = audioSeconds > 0 ? Math.round((pacedWords / audioSeconds) * 60) : 0;
  const savedMinutes = Math.max(0, words / TYPING_WPM - words / CONSERVATIVE_SPEAKING_WPM);
  const currentStreak = currentInsightStreak(recordings);
  const longestStreak = longestInsightStreak(recordings);

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

  const activeAliases = aliases.filter((alias) => alias.active);
  const appliedFixes = activeAliases.reduce((sum, alias) => sum + alias.use_count, 0);
  const manualTerms = vocabulary.filter((term) => term.source === "manual").length;
  const recentTerms = [...vocabulary].sort((a, b) => b.last_used - a.last_used).slice(0, 6);
  const topAliases = [...activeAliases].sort((a, b) => b.use_count - a.use_count).slice(0, 6);

  return (
    <ScrollArea className="h-full">
      <div className="insights-page">
        <header className="insights-header">
          <div>
            <h1>Insights</h1>
            <p className="insights-subtitle">Your momentum, on this Mac</p>
          </div>
          <div className="insights-tabs" role="tablist" aria-label="Insights sections">
            <button type="button" role="tab" aria-selected={tab === "activity"} onClick={() => setTab("activity")}>Activity</button>
            <button type="button" role="tab" aria-selected={tab === "learning"} onClick={() => setTab("learning")}>Learning</button>
          </div>
        </header>

        {loading ? <InsightsSkeleton /> : tab === "activity" ? (
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
        ) : (
          <LearningTab
            vocabulary={vocabulary}
            activeAliases={activeAliases}
            appliedFixes={appliedFixes}
            manualTerms={manualTerms}
            recentTerms={recentTerms}
            topAliases={topAliases}
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
  const totalDays = 16 * 7;
  const today = new Date();
  today.setHours(0, 0, 0, 0);
  const start = new Date(today);
  start.setDate(today.getDate() - (totalDays - 1));
  const byDay = new Map<string, number>();
  for (const recording of recordings) {
    const key = localDayKey(recording.timestamp_ms);
    byDay.set(key, (byDay.get(key) ?? 0) + recording.word_count);
  }
  const days = Array.from({ length: totalDays }, (_, index) => {
    const date = new Date(start);
    date.setDate(start.getDate() + index);
    const words = byDay.get(localDayKey(date.getTime())) ?? 0;
    return { date, words };
  });
  const maxWords = Math.max(1, ...days.map((day) => day.words));

  return (
    <div className="insights-card insights-heat-card">
      <div className="insights-card-heading"><div><p className="insights-kicker">Last 16 weeks</p><h2>Consistency</h2></div><Target size={17} /></div>
      <div className="insights-streak-line"><strong>{currentStreak} day streak</strong><span>Longest: {longestStreak} days</span></div>
      <div className="insights-heatmap" aria-label="Daily dictated words over the last 16 weeks">
        {days.map((day) => {
          const ratio = day.words / maxWords;
          const level = day.words === 0 ? 0 : ratio < 0.25 ? 1 : ratio < 0.5 ? 2 : ratio < 0.75 ? 3 : 4;
          return <span key={day.date.toISOString()} className={`heat-level-${level}`} title={`${day.date.toLocaleDateString()}: ${day.words.toLocaleString()} words`} />;
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

function LearningTab({ vocabulary, activeAliases, appliedFixes, manualTerms, recentTerms, topAliases }: { vocabulary: VocabRow[]; activeAliases: VocabAlias[]; appliedFixes: number; manualTerms: number; recentTerms: VocabRow[]; topAliases: VocabAlias[] }) {
  const autoTerms = vocabulary.filter((term) => term.source === "auto").length;
  return (
    <div className="insights-reveal" role="tabpanel">
      <div className="insights-learning-intro"><div><p className="insights-kicker">Private, durable memory</p><h2>AirNote is adapting to the words that matter to you.</h2><p>This is based only on vocabulary and correction rules stored by AirNote, not inferred personality scores.</p></div><span className="insights-learning-orbit"><Sparkles /></span></div>
      <section className="insights-metric-grid insights-learning-metrics">
        <MetricCard icon={<BookOpen size={15} />} label="Vocabulary" value={vocabulary.length.toLocaleString()} detail={`${manualTerms} added by you · ${autoTerms} learned`} />
        <MetricCard icon={<Languages size={15} />} label="Active corrections" value={activeAliases.length.toLocaleString()} detail="Approved mishearing fixes" />
        <MetricCard icon={<Check size={15} />} label="Fixes applied" value={appliedFixes.toLocaleString()} detail="Times active rules were used" />
      </section>
      <section className="insights-two-column">
        <div className="insights-card">
          <div className="insights-card-heading"><div><p className="insights-kicker">Recently used</p><h2>Your vocabulary</h2></div><BookOpen size={17} /></div>
          {recentTerms.length ? <div className="insights-term-list">{recentTerms.map((term) => <div key={term.term} className="insights-term-row"><span>{term.term.slice(0, 1).toUpperCase()}</span><div><strong>{term.term}</strong><small>{term.meaning || (term.source === "manual" ? "Added by you" : "Learned from corrections")}</small></div><em>{term.use_count} uses</em></div>)}</div> : <EmptyInsight text="Your vocabulary grows when you add a term or approve a correction." />}
        </div>
        <div className="insights-card">
          <div className="insights-card-heading"><div><p className="insights-kicker">Working for you</p><h2>Top correction rules</h2></div><ArrowUpRight size={17} /></div>
          {topAliases.length ? <div className="insights-alias-list">{topAliases.map((alias) => <div key={`${alias.transcript_form}-${alias.correct_form}`} className="insights-alias-row"><span>{alias.transcript_form}</span><ArrowUpRight size={13} /><strong>{alias.correct_form}</strong><em>{alias.use_count}×</em></div>)}</div> : <EmptyInsight text="Approved pronunciation fixes will appear here after AirNote learns one." />}
        </div>
      </section>
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
