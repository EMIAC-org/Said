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
