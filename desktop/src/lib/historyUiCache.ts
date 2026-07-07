import type { Recording } from "@/types";
import { isTauriRuntime, listHistory, onVoiceDone } from "@/lib/invoke";

const FRESH_FOR_MS = 5 * 60_000;
const POST_DICTATION_REFRESH_DELAYS_MS = [1_000, 5_000];
const RETRY_DELAYS_MS = [2_000, 5_000, 10_000, 30_000];

type Listener = () => void;
type Unsubscribe = () => void;

export interface HistoryCacheSnapshot {
  data: Recording[] | null | undefined;
  refreshing: boolean;
  loadingMore: boolean;
  stale: boolean;
  updatedAt: number | null;
  hasMore: boolean;
}

let historyData: Recording[] | null | undefined;
let historyUpdatedAt: number | null = null;
let historyStale = true;
let historyRefreshing = false;
let historyLoadingMore = false;
let historyHasMore = false;
let historyFailureCount = 0;
let historyInFlight: Promise<Recording[] | null> | null = null;
let historyLoadMoreInFlight: Promise<Recording[]> | null = null;
let historyRetryTimer: number | null = null;
let eventsInstalled = false;

const listeners = new Set<Listener>();

function now() {
  return Date.now();
}

function isFresh() {
  return !!historyUpdatedAt && !historyStale && now() - historyUpdatedAt < FRESH_FOR_MS;
}

function notify() {
  for (const listener of listeners) listener();
}

export function getHistorySnapshot(): HistoryCacheSnapshot {
  return {
    data: historyData,
    refreshing: historyRefreshing,
    loadingMore: historyLoadingMore,
    stale: historyStale,
    updatedAt: historyUpdatedAt,
    hasMore: historyHasMore,
  };
}

export function subscribeHistory(listener: Listener): Unsubscribe {
  ensureHistoryCacheEvents();
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0) clearHistoryRetry();
  };
}

export function refreshHistoryCache(
  pageSize: number,
  options: { force?: boolean } = {},
): Promise<Recording[] | null> {
  ensureHistoryCacheEvents();
  if (!options.force && isFresh()) {
    return Promise.resolve(historyData ?? null);
  }
  if (historyInFlight) return historyInFlight;

  historyRefreshing = true;
  notify();
  historyInFlight = listHistory(pageSize)
    .then((rows) => {
      historyData = rows;
      historyUpdatedAt = now();
      historyHasMore = rows.length >= pageSize;
      historyStale = false;
      historyFailureCount = 0;
      clearHistoryRetry();
      return rows;
    })
    .catch(() => {
      if (historyData === undefined) historyData = null;
      historyStale = true;
      scheduleHistoryRetry(pageSize);
      return historyData ?? null;
    })
    .finally(() => {
      historyRefreshing = false;
      historyInFlight = null;
      notify();
    });
  return historyInFlight;
}

export function loadMoreHistoryCache(pageSize: number): Promise<Recording[]> {
  ensureHistoryCacheEvents();
  if (historyLoadMoreInFlight) return historyLoadMoreInFlight;
  const oldest = historyData?.[historyData.length - 1]?.timestamp_ms;
  if (!oldest) return Promise.resolve([]);

  historyLoadingMore = true;
  notify();
  historyLoadMoreInFlight = listHistory(pageSize, oldest)
    .then((older) => {
      if (older.length > 0) {
        const current = historyData ?? [];
        const seen = new Set(current.map((r) => r.id));
        historyData = [...current, ...older.filter((r) => !seen.has(r.id))];
        historyUpdatedAt = now();
      }
      historyHasMore = older.length >= pageSize;
      return older;
    })
    .finally(() => {
      historyLoadingMore = false;
      historyLoadMoreInFlight = null;
      notify();
    });
  return historyLoadMoreInFlight;
}

export function removeHistoryCached(id: string) {
  if (!historyData) return;
  historyData = historyData.filter((r) => r.id !== id);
  notify();
}

export function insertHistoryCached(recording: Recording) {
  const current = historyData ?? [];
  historyData = [...current.filter((r) => r.id !== recording.id), recording].sort(
    (a, b) => b.timestamp_ms - a.timestamp_ms,
  );
  historyUpdatedAt = now();
  notify();
}

export function replaceHistoryCached(recordings: Recording[]) {
  historyData = recordings;
  historyUpdatedAt = now();
  historyHasMore = recordings.length >= 50;
  notify();
}

export function clearHistoryCached() {
  historyData = [];
  historyUpdatedAt = now();
  historyHasMore = false;
  notify();
}

function markHistoryStale() {
  historyStale = true;
  notify();
}

function refreshObservedHistory(pageSize = 50) {
  if (listeners.size > 0) void refreshHistoryCache(pageSize, { force: true });
}

function scheduleHistoryRetry(pageSize: number) {
  if (historyRetryTimer !== null || listeners.size === 0) return;
  const delay = RETRY_DELAYS_MS[Math.min(historyFailureCount, RETRY_DELAYS_MS.length - 1)];
  historyFailureCount += 1;
  historyRetryTimer = window.setTimeout(() => {
    historyRetryTimer = null;
    if (listeners.size > 0 && historyStale) {
      void refreshHistoryCache(pageSize, { force: true });
    }
  }, delay);
}

function clearHistoryRetry() {
  if (historyRetryTimer === null) return;
  window.clearTimeout(historyRetryTimer);
  historyRetryTimer = null;
}

function ensureHistoryCacheEvents() {
  if (eventsInstalled || !isTauriRuntime()) return;
  eventsInstalled = true;
  onVoiceDone(() => {
    markHistoryStale();
    for (const delay of POST_DICTATION_REFRESH_DELAYS_MS) {
      window.setTimeout(refreshObservedHistory, delay);
    }
  });
  window.addEventListener("focus", () => {
    if (historyStale || !historyUpdatedAt) refreshObservedHistory();
  });
  window.addEventListener("online", () => {
    if (historyStale || !historyUpdatedAt) refreshObservedHistory();
  });
}
