export interface InsightHistoryPoint {
  timestamp_ms: number;
  word_count: number;
}

const DAY_MS = 86_400_000;

function localDayIndex(timestampMs: number): number {
  const date = new Date(timestampMs);
  // Convert local calendar components to a UTC ordinal. Using the local
  // midnight timestamp directly breaks streaks across DST transitions.
  return Math.floor(Date.UTC(date.getFullYear(), date.getMonth(), date.getDate()) / DAY_MS);
}

export function localDayKey(timestampMs: number): string {
  const date = new Date(timestampMs);
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
}

export function currentInsightStreak(points: InsightHistoryPoint[]): number {
  const days = new Set(points.map((point) => localDayIndex(point.timestamp_ms)));
  let cursor = localDayIndex(Date.now());
  if (!days.has(cursor) && !days.has(cursor - 1)) return 0;
  if (!days.has(cursor)) cursor -= 1;

  let streak = 0;
  while (days.has(cursor)) {
    streak += 1;
    cursor -= 1;
  }
  return streak;
}

export function longestInsightStreak(points: InsightHistoryPoint[]): number {
  const days = [...new Set(points.map((point) => localDayIndex(point.timestamp_ms)))].sort(
    (a, b) => a - b,
  );
  let longest = 0;
  let current = 0;
  let previous: number | null = null;
  for (const day of days) {
    current = previous !== null && day === previous + 1 ? current + 1 : 1;
    longest = Math.max(longest, current);
    previous = day;
  }
  return longest;
}

export interface HeatmapDay {
  key: string;
  date: Date;
  words: number;
  /** Future placeholder cell in the current week (after today). */
  isFuture: boolean;
}

/**
 * Build a GitHub-style contribution grid: `weeks` columns × 7 rows (Sun→Sat),
 * column-major. The rightmost column is the current week (ending Saturday).
 */
export function buildHeatmapDays(
  points: InsightHistoryPoint[],
  weeks = 16,
  nowMs = Date.now(),
): HeatmapDay[] {
  const totalDays = weeks * 7;
  const now = new Date(nowMs);
  now.setHours(0, 0, 0, 0);

  // Anchor on the upcoming Saturday so each column is a real Sun–Sat week.
  const todayDow = now.getDay(); // 0=Sun .. 6=Sat
  const lastSat = new Date(now);
  lastSat.setDate(lastSat.getDate() + (6 - todayDow));
  const start = new Date(lastSat);
  start.setDate(start.getDate() - (totalDays - 1));

  const byDay = new Map<string, number>();
  for (const point of points) {
    const key = localDayKey(point.timestamp_ms);
    byDay.set(key, (byDay.get(key) ?? 0) + point.word_count);
  }

  return Array.from({ length: totalDays }, (_, index) => {
    const date = new Date(start);
    date.setDate(start.getDate() + index);
    const key = localDayKey(date.getTime());
    const isFuture = date.getTime() > now.getTime();
    return {
      key,
      date,
      words: isFuture ? 0 : (byDay.get(key) ?? 0),
      isFuture,
    };
  });
}

/** Longest streak among days that fall inside the heatmap window. */
export function longestInsightStreakInWindow(
  points: InsightHistoryPoint[],
  weeks = 16,
  nowMs = Date.now(),
): number {
  const windowStart = (() => {
    const now = new Date(nowMs);
    now.setHours(0, 0, 0, 0);
    const todayDow = now.getDay();
    const lastSat = new Date(now);
    lastSat.setDate(lastSat.getDate() + (6 - todayDow));
    const start = new Date(lastSat);
    start.setDate(start.getDate() - (weeks * 7 - 1));
    return start.getTime();
  })();
  return longestInsightStreak(points.filter((p) => p.timestamp_ms >= windowStart));
}
