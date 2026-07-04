import React, { useMemo, useState, useEffect } from "react";
import { Info, CheckCircle2, PenLine, Monitor } from "lucide-react";
import { ScrollArea } from "@/components/ui/scroll-area";
import { cn } from "@/lib/utils";
import { listAppUsage, getAppIdentity, listSiteUsage, getFavicon } from "@/lib/invoke";
import type { AppSnapshot, HistoryItem, AppIdentity, AppUsageRow, SiteUsageRow } from "@/types";

interface InsightsViewProps {
  snapshot: AppSnapshot | null;
}

// ── WPM Gauge (refined for dark theme) ───────────────────────────────────────

function WpmGauge({ wpm }: { wpm: number }) {
  const maxWpm        = 250;
  const pct           = Math.min(wpm / maxWpm, 1);
  const r             = 52;
  const cx            = 70;
  const cy            = 70;
  const circumference = Math.PI * r;
  const dashOffset    = circumference * (1 - pct);
  const topPct        =
    wpm >= 200 ? "5%" : wpm >= 175 ? "10%" : wpm >= 150 ? "20%" : wpm >= 120 ? "35%" : "50%";

  return (
    <div className="flex flex-col items-center mt-4">
      <svg width="140" height="80" viewBox="0 0 140 80">
        <defs>
          <linearGradient id="wpmGrad" x1="0" y1="0" x2="140" y2="0" gradientUnits="userSpaceOnUse">
            <stop offset="0%"   stopColor="hsl(var(--accent-violet))" />
            <stop offset="100%" stopColor="hsl(var(--primary))" />
          </linearGradient>
        </defs>
        {/* Track — uses muted-foreground so visible in both modes */}
        <path
          d={`M ${cx - r},${cy} A ${r},${r} 0 0 1 ${cx + r},${cy}`}
          strokeWidth="8" fill="none"
          stroke="hsl(var(--muted-foreground) / 0.18)"
          strokeLinecap="round"
        />
        {/* Fill — violet → mint gradient */}
        <path
          d={`M ${cx - r},${cy} A ${r},${r} 0 0 1 ${cx + r},${cy}`}
          strokeWidth="8" fill="none"
          stroke="url(#wpmGrad)"
          strokeLinecap="round"
          strokeDasharray={circumference}
          strokeDashoffset={dashOffset}
          style={{ transition: "stroke-dashoffset 0.6s ease" }}
        />
        <text x={cx} y={cy - 8} textAnchor="middle" fill="hsl(var(--muted-foreground))" fontSize="11">
          Top
        </text>
        <text x={cx} y={cy + 6} textAnchor="middle"
              fill="hsl(var(--foreground))" fontSize="15" fontWeight="bold"
              style={{ fontVariantNumeric: "tabular-nums" }}>
          {topPct}
        </text>
      </svg>
    </div>
  );
}

// ── Heatmap helpers ────────────────────────────────────────────────────────────

const DAYS      = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const COL_COUNT = 14;

function heatmapMonths(): string[] {
  const now  = new Date();
  const seen = new Set<string>();
  const result: string[] = [];
  for (let w = COL_COUNT - 1; w >= 0; w--) {
    const d = new Date(now.getTime() - w * 7 * 86_400_000);
    const label = d.toLocaleDateString("en-US", { month: "short" });
    if (!seen.has(label)) {
      seen.add(label);
      result.push(label);
    }
  }
  return result;
}

// Local-calendar day index — UTC `floor(ms / DAY)` would split IST evenings
// onto the wrong cell (UTC+5:30 means 1am IST is actually 7:30pm UTC the
// previous day). Using the LOCAL midnight gives consistent same-day grouping.
function localDayIdx(ms: number): number {
  const d = new Date(ms);
  const localMidnight = new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  return Math.floor(localMidnight / 86_400_000);
}

function buildDayMap(history: HistoryItem[]): Map<number, number> {
  const map = new Map<number, number>();
  for (const item of history) {
    const day = localDayIdx(item.timestamp_ms);
    map.set(day, (map.get(day) ?? 0) + item.word_count);
  }
  return map;
}

function wordCountToLevel(words: number): 0 | 1 | 2 | 3 | 4 {
  if (words === 0)  return 0;
  if (words < 20)   return 1;
  if (words < 60)   return 2;
  if (words < 150)  return 3;
  return 4;
}

// ── Accuracy helpers ───────────────────────────────────────────────────────────

function computeAccuracy(history: HistoryItem[]) {
  if (history.length === 0) return { allTime: 0, recent: 0, accepted: 0, edited: 0, total: 0 };
  const accepted = history.filter((h) => (h.edit_count ?? 0) === 0).length;
  const edited   = history.length - accepted;
  const allTime  = Math.round((accepted / history.length) * 100);

  const weekAgo  = Date.now() - 7 * 86_400_000;
  const recent7  = history.filter((h) => h.timestamp_ms >= weekAgo);
  const recent   = recent7.length > 0
    ? Math.round((recent7.filter((h) => (h.edit_count ?? 0) === 0).length / recent7.length) * 100)
    : 0;

  return { allTime, recent, accepted, edited, total: history.length };
}

// ── Apps you dictate in ───────────────────────────────────────────────────────

interface AppRow extends AppUsageRow {
  identity: AppIdentity | null;
}

/** Prettify a raw app key when we couldn't resolve a friendly name:
 *  bundle-id `com.tinyspeck.slack` → "Slack"; exe path → stem without extension. */
function formatKey(key: string): string {
  const t = key.trim();
  if (t.includes("/") || t.includes("\\")) {
    const seg = t.split(/[\\/]/).pop() ?? t;
    return seg.replace(/\.(app|exe)$/i, "");
  }
  if (!t.includes(".")) return t;
  const seg = t.split(".").pop() ?? t;
  return seg.charAt(0).toUpperCase() + seg.slice(1);
}

/** "Apps you dictate in" — per-app usage with real icons, names and categories.
 *  This is the knowledge-base showpiece: it maps every recording to where it was
 *  typed. Populates as new dictations are captured (older ones have no app). */
function AppUsageSection() {
  const [rows, setRows] = useState<AppRow[] | null>(null);

  useEffect(() => {
    let alive = true;
    void listAppUsage().then(async (usage) => {
      const top = usage.slice(0, 8);
      const withIdentity = await Promise.all(
        top.map(async (u) => ({ ...u, identity: await getAppIdentity(u.app) })),
      );
      if (alive) setRows(withIdentity);
    });
    return () => { alive = false; };
  }, []);

  if (rows === null) return null; // loading — stay quiet until resolved

  const maxCount = rows.reduce((m, r) => Math.max(m, r.count), 0) || 1;

  return (
    <div className="panel p-5 mt-3">
      <div className="flex items-baseline justify-between mb-5">
        <h2 className="text-[14px] font-semibold text-foreground">Apps you dictate in</h2>
        <span className="section-label">{rows.length} app{rows.length === 1 ? "" : "s"}</span>
      </div>

      {rows.length === 0 ? (
        <p className="text-[12px] text-muted-foreground">
          Dictate into your apps and they’ll show up here — every recording is mapped
          to where you typed it.
        </p>
      ) : (
        <div className="space-y-3">
          {rows.map((r) => {
            const name = r.identity?.name ?? formatKey(r.app);
            const pct = Math.round((r.count / maxCount) * 100);
            return (
              <div key={r.app} className="flex items-center gap-3">
                <div
                  className="w-9 h-9 flex-shrink-0 rounded-lg overflow-hidden flex items-center justify-center"
                  style={{ background: "hsl(var(--surface-4))" }}
                >
                  {r.identity?.icon ? (
                    <img src={r.identity.icon} alt={name} className="w-full h-full object-contain" draggable={false} />
                  ) : (
                    <Monitor size={16} className="text-muted-foreground" />
                  )}
                </div>
                <div className="flex-1 min-w-0">
                  <div className="flex items-baseline justify-between gap-2">
                    <span className="text-[13px] font-medium text-foreground truncate">{name}</span>
                    <span className="text-[11px] text-muted-foreground tabular-nums flex-shrink-0">
                      {r.count} · {r.total_words.toLocaleString()} words
                    </span>
                  </div>
                  <div className="mt-1.5 flex items-center gap-2">
                    <div className="flex-1 h-1.5 rounded-full overflow-hidden" style={{ background: "hsl(var(--surface-4))" }}>
                      <div className="h-full rounded-full" style={{ width: `${pct}%`, background: "hsl(var(--accent-violet))" }} />
                    </div>
                    {r.identity?.category && (
                      <span
                        className="text-[10px] px-1.5 py-0.5 rounded-md flex-shrink-0"
                        style={{ background: "hsl(var(--surface-4))", color: "hsl(var(--muted-foreground))" }}
                      >
                        {r.identity.category}
                      </span>
                    )}
                  </div>
                </div>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

// ── Sites you dictate in (browser context) ────────────────────────────────────

const faviconCache = new Map<string, string | null>();
const faviconInflight = new Map<string, Promise<string | null>>();
function resolveFavicon(host: string): Promise<string | null> {
  if (faviconCache.has(host)) return Promise.resolve(faviconCache.get(host) ?? null);
  const inflight = faviconInflight.get(host);
  if (inflight) return inflight;
  const p = getFavicon(host).then((url) => {
    faviconCache.set(host, url);
    faviconInflight.delete(host);
    return url;
  });
  faviconInflight.set(host, p);
  return p;
}

/** Deterministic tile color from a host, for the no-favicon fallback. */
function hostTint(host: string): string {
  let h = 0;
  for (let i = 0; i < host.length; i++) h = (h * 31 + host.charCodeAt(i)) >>> 0;
  return `hsl(${h % 360} 45% 42%)`;
}

interface SiteRow extends SiteUsageRow {
  favicon: string | null;
}

/** "Sites you dictate in" — per-site usage with real favicons. Only populates
 *  when the opt-in browser-context feature is on and you've dictated into a
 *  browser; hidden entirely otherwise. */
function SitesUsageSection() {
  const [rows, setRows] = useState<SiteRow[] | null>(null);

  useEffect(() => {
    let alive = true;
    void listSiteUsage().then(async (usage) => {
      const top = usage.slice(0, 8);
      const withIcons = await Promise.all(
        top.map(async (u) => ({ ...u, favicon: await resolveFavicon(u.host) })),
      );
      if (alive) setRows(withIcons);
    });
    return () => { alive = false; };
  }, []);

  // Hidden until there's something to show (keeps the page clean pre-opt-in).
  if (rows === null || rows.length === 0) return null;

  const maxCount = rows.reduce((m, r) => Math.max(m, r.count), 0) || 1;

  return (
    <div className="panel p-5 mt-3">
      <div className="flex items-baseline justify-between mb-5">
        <h2 className="text-[14px] font-semibold text-foreground">Sites you dictate in</h2>
        <span className="section-label">{rows.length} site{rows.length === 1 ? "" : "s"}</span>
      </div>
      <div className="space-y-3">
        {rows.map((r) => {
          const pct = Math.round((r.count / maxCount) * 100);
          return (
            <div key={r.host} className="flex items-center gap-3">
              <div
                className="w-9 h-9 flex-shrink-0 rounded-lg overflow-hidden flex items-center justify-center text-[13px] font-semibold text-white"
                style={{ background: r.favicon ? "hsl(var(--surface-4))" : hostTint(r.host) }}
              >
                {r.favicon ? (
                  <img src={r.favicon} alt={r.host} className="w-full h-full object-contain" draggable={false} />
                ) : (
                  (r.host[0] ?? "?").toUpperCase()
                )}
              </div>
              <div className="flex-1 min-w-0">
                <div className="flex items-baseline justify-between gap-2">
                  <span className="text-[13px] font-medium text-foreground truncate">{r.host}</span>
                  <span className="text-[11px] text-muted-foreground tabular-nums flex-shrink-0">
                    {r.count}
                  </span>
                </div>
                <div className="mt-1.5 h-1.5 rounded-full overflow-hidden" style={{ background: "hsl(var(--surface-4))" }}>
                  <div className="h-full rounded-full" style={{ width: `${pct}%`, background: "hsl(var(--secondary))" }} />
                </div>
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}

// ── View ───────────────────────────────────────────────────────────────────────

export function InsightsView({ snapshot }: InsightsViewProps) {
  const history       = snapshot?.history ?? [];
  const wpm           = snapshot?.avg_wpm ?? 0;
  const words         = snapshot?.total_words ?? 0;
  const streak        = snapshot?.daily_streak ?? 0;

  const dayMap        = useMemo(() => buildDayMap(history), [history]);
  const accuracy      = useMemo(() => computeAccuracy(history), [history]);
  // Local day index — matches dayMap keys built from `localDayIdx`.
  const todayUnixDay  = localDayIdx(Date.now());

  return (
    <ScrollArea className="h-full">
      <div className="p-5 pb-12 mx-auto overflow-hidden" style={{ maxWidth: "min(900px, 100%)" }}>

        {/* ── Header ──────────────────────────────── */}
        <div className="mb-7">
          <h1 className="text-[24px] font-bold tracking-tight text-foreground leading-tight">
            Insights
          </h1>
          <p className="text-[12.5px] text-muted-foreground mt-1 flex items-center gap-2">
            <span
              className="inline-block w-1.5 h-1.5 rounded-full"
              style={{
                background: "hsl(var(--accent-violet))",
                boxShadow:  "0 0 8px hsl(var(--accent-violet) / 0.5)",
              }}
            />
            Your recording analytics · {history.length} session{history.length === 1 ? "" : "s"}
          </p>
        </div>

        {/* ── Top grid: responsive ─────────────────── */}
        <div className="grid gap-3 mb-3" style={{ gridTemplateColumns: "repeat(auto-fit, minmax(200px, 1fr))" }}>

          {/* WPM gauge */}
          <div className="panel p-5">
            <div className="text-[32px] font-bold tracking-tight text-foreground leading-none tabular-nums">
              {wpm || "—"}
            </div>
            <div className="section-label mt-2">Words per minute</div>
            {wpm > 0 ? (
              <WpmGauge wpm={wpm} />
            ) : (
              <p className="text-[12px] text-muted-foreground mt-4 leading-relaxed">
                Record something to see your speed.
              </p>
            )}
          </div>

          {/* Sessions card */}
          <div className="panel p-5">
            <div className="text-[32px] font-bold tracking-tight text-foreground leading-none tabular-nums">
              {history.length}
            </div>
            <div className="section-label mt-2 mb-4">Sessions polished</div>
            <div className="mt-2" />
            <div className="space-y-3">
              <div className="flex items-center justify-between text-[13px]">
                <span className="text-foreground tabular-nums">{words.toLocaleString()} words total</span>
                <Info size={11} className="text-muted-foreground/50" />
              </div>
              <div className="flex items-center justify-between text-[13px]">
                <span className="text-foreground tabular-nums">{streak} day streak</span>
                <Info size={11} className="text-muted-foreground/50" />
              </div>
            </div>
          </div>

          {/* Total words */}
          <div
            className="panel p-5 relative overflow-hidden"
            style={{
              background:
                "linear-gradient(135deg, hsl(var(--surface-3)) 0%, hsl(var(--surface-3)) 60%, hsl(var(--accent-violet) / 0.10) 100%)",
            }}
          >
            <div
              aria-hidden
              className="absolute pointer-events-none"
              style={{
                right: -60, bottom: -60, width: 180, height: 180, borderRadius: "50%",
                background: "radial-gradient(circle, hsl(var(--accent-violet) / 0.20) 0%, transparent 70%)",
              }}
            />
            <div
              className="relative font-bold tracking-tight leading-none tabular-nums"
              style={{
                fontSize: 32,
                background: "linear-gradient(135deg, hsl(var(--foreground)) 0%, hsl(var(--accent-violet)) 100%)",
                WebkitBackgroundClip: "text",
                WebkitTextFillColor: "transparent",
                backgroundClip: "text",
              }}
            >
              {words.toLocaleString()}
            </div>
            <div className="section-label mt-2 mb-4 relative">Total words dictated</div>
            <div className="mt-2" />
            <div className="text-[13px] text-foreground relative">
              Desktop · {snapshot?.platform === "windows" ? "Windows" : snapshot?.platform === "macos" ? "macOS" : "Desktop"}
            </div>
            <div className="text-[11px] text-muted-foreground mt-0.5 tabular-nums relative">
              {words.toLocaleString()} polished words across {history.length} session{history.length === 1 ? "" : "s"}
            </div>
          </div>
        </div>

        {/* ── Bottom grid: responsive ────────────────── */}
        <div className="grid gap-3" style={{ gridTemplateColumns: "repeat(auto-fit, minmax(280px, 1fr))" }}>

          {/* Accuracy card */}
          <div className="panel p-5">
            <div className="flex items-baseline justify-between mb-5">
              <h2 className="text-[14px] font-semibold text-foreground">Accuracy</h2>
              <span className="section-label">{accuracy.total} sessions</span>
            </div>

            {accuracy.total === 0 ? (
              <p className="text-[12px] text-muted-foreground">
                Record something to see your accuracy.
              </p>
            ) : (
              <>
                {/* Big accuracy ring */}
                <div className="flex flex-col items-center mb-5">
                  <svg width="100" height="100" viewBox="0 0 100 100">
                    <circle
                      cx="50" cy="50" r="42"
                      strokeWidth="7" fill="none"
                      stroke="hsl(var(--surface-4))"
                    />
                    <circle
                      cx="50" cy="50" r="42"
                      strokeWidth="7" fill="none"
                      stroke="hsl(var(--primary))"
                      strokeLinecap="round"
                      strokeDasharray={2 * Math.PI * 42}
                      strokeDashoffset={2 * Math.PI * 42 * (1 - accuracy.allTime / 100)}
                      transform="rotate(-90 50 50)"
                      style={{ transition: "stroke-dashoffset 0.6s ease" }}
                    />
                    <text
                      x="50" y="46" textAnchor="middle"
                      fill="hsl(var(--foreground))"
                      fontSize="22" fontWeight="bold"
                      style={{ fontVariantNumeric: "tabular-nums" }}
                    >
                      {accuracy.allTime}%
                    </text>
                    <text
                      x="50" y="62" textAnchor="middle"
                      fill="hsl(var(--muted-foreground))"
                      fontSize="10"
                    >
                      accepted
                    </text>
                  </svg>
                </div>

                {/* Stats rows */}
                <div className="space-y-3">
                  <div className="flex items-center justify-between text-[13px]">
                    <span className="flex items-center gap-2 text-foreground">
                      <CheckCircle2 size={13} className="text-muted-foreground" />
                      Accepted as-is
                    </span>
                    <span className="tabular-nums font-semibold text-foreground">
                      {accuracy.accepted}
                    </span>
                  </div>
                  <div className="flex items-center justify-between text-[13px]">
                    <span className="flex items-center gap-2 text-foreground">
                      <PenLine size={13} className="text-muted-foreground" />
                      Edited after paste
                    </span>
                    <span className="tabular-nums font-semibold text-foreground">
                      {accuracy.edited}
                    </span>
                  </div>
                  {accuracy.recent > 0 && (
                    <div
                      className="mt-2 pt-3 flex items-center justify-between text-[12px]"
                      style={{ borderTop: "1px solid hsl(var(--border))" }}
                    >
                      <span className="text-muted-foreground">Last 7 days</span>
                      <span className="tabular-nums font-semibold text-foreground">
                        {accuracy.recent}%
                      </span>
                    </div>
                  )}
                </div>
              </>
            )}
          </div>

          {/* Heatmap */}
          <div className="tile p-5 overflow-hidden" style={{ minWidth: 0 }}>
            <div className="flex items-baseline justify-between mb-4">
              <h2 className="text-[14px] font-semibold text-foreground">
                {streak > 0 ? `${streak} day streak` : "No streak yet"}
              </h2>
              <span className="section-label">Best · {streak}d</span>
            </div>

            {/* Month labels */}
            <div className="flex items-center justify-between text-[11px] text-muted-foreground mb-3 px-1">
              {heatmapMonths().map((m, i, arr) => (
                <span key={m} className={cn(i === arr.length - 1 && "text-foreground font-medium")}>{m}</span>
              ))}
            </div>

            {/* Grid — proper week × weekday calendar:
                · each column = one week (oldest → newest, left → right)
                · each row    = one weekday (Sun…Sat)
                · cells anchored to the most-recent Sunday so columns align */}
            <div
              className="grid gap-1"
              style={{ gridTemplateColumns: `32px repeat(${COL_COUNT}, minmax(0, 1fr))` }}
            >
              {(() => {
                const todayDow     = new Date().getDay();   // local DOW
                const lastSundayIx = todayUnixDay - todayDow;

                return DAYS.map((day, dayOfWeek) => (
                  <React.Fragment key={day}>
                    <span className="text-[10px] text-muted-foreground flex items-center">{day}</span>
                    {Array.from({ length: COL_COUNT }, (_, col) => {
                      const weeksAgo  = COL_COUNT - 1 - col;
                      const cellDay   = lastSundayIx - weeksAgo * 7 + dayOfWeek;
                      const isFuture  = cellDay > todayUnixDay;
                      const isCurrent = cellDay === todayUnixDay;
                      const cellWords = isFuture ? 0 : (dayMap.get(cellDay) ?? 0);
                      const level     = isFuture ? 0 : wordCountToLevel(cellWords);
                      return (
                        <span
                          key={col}
                          className={cn(
                            "aspect-square rounded-[3px]",
                            isCurrent ? "heat-current" : `heat-${level}`
                          )}
                          style={{ opacity: isFuture ? 0.3 : 1 }}
                          title={!isFuture && cellWords > 0
                            ? `${cellWords} words on ${(() => {
                                const daysAgo = todayUnixDay - cellDay;
                                const d = new Date();
                                d.setHours(0, 0, 0, 0);
                                d.setDate(d.getDate() - daysAgo);
                                return d.toLocaleDateString("en-US", { month: "short", day: "numeric" });
                              })()}`
                            : undefined}
                        />
                      );
                    })}
                  </React.Fragment>
                ));
              })()}
            </div>

            {/* Legend */}
            <div className="flex items-center justify-between mt-4 text-[10px] text-muted-foreground">
              <div className="flex items-center gap-1.5">
                <span>More</span>
                {([4, 3, 2, 1, 0] as const).map((l) => (
                  <span key={l} className={cn("w-3 h-3 rounded-[2px]", `heat-${l}`)} />
                ))}
                <span>Less</span>
              </div>
              <div className="flex items-center gap-1.5">
                <span className="w-3 h-3 rounded-[2px] heat-current" />
                <span>Today</span>
              </div>
            </div>
          </div>
        </div>

        {/* ── Apps you dictate in ─────────────────────── */}
        <AppUsageSection />

        {/* ── Sites you dictate in (browser context) ──── */}
        <SitesUsageSection />

      </div>
    </ScrollArea>
  );
}
