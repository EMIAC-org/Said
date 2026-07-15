import { listHistory, onVoiceDone } from "@/lib/invoke";
import type { Recording } from "@/types";

const FRESH_FOR_MS = 60_000;
const EVENT_REFRESH_DELAY_MS = 250;
const EVENT_REFRESH_LIMIT = 300;
const SERVER_PAGE_LIMIT = 200;

type Listener = () => void;

export interface HistoryCacheSnapshot {
  recordings: Recording[] | undefined;
  refreshing: boolean;
  stale: boolean;
  updatedAt: number | null;
}

let recordings: Recording[] | undefined;
let exhausted = false;
let updatedAt: number | null = null;
let stale = true;
let refreshing = false;
interface HistoryFetchResult {
  rows: Recording[];
  reachedEnd: boolean;
}

let inFlight: Promise<HistoryFetchResult> | null = null;
let eventsInstalled = false;
let refreshTimer: ReturnType<typeof setTimeout> | undefined;
let invalidationVersion = 0;
const listeners = new Set<Listener>();

function isFresh() {
  return !!updatedAt && !stale && Date.now() - updatedAt < FRESH_FOR_MS;
}

function hasCoverage(limit: number) {
  return !!recordings && (recordings.length >= limit || exhausted);
}

function notify() {
  for (const listener of listeners) listener();
}

function mergeHistoryRows(cached: Recording[], authoritative: Recording[]): Recording[] {
  const byId = new Map(cached.map((recording) => [recording.id, recording]));
  // The backend response may contain metadata that was missing from an earlier
  // snapshot (for example target_app). Always let it replace the cached row.
  for (const recording of authoritative) byId.set(recording.id, recording);
  return [...byId.values()].sort((a, b) => b.timestamp_ms - a.timestamp_ms);
}

async function fetchHistoryWindow(limit: number, before?: number): Promise<HistoryFetchResult> {
  const target = Math.max(1, limit);
  let cursor = before;
  let rows: Recording[] = [];
  let reachedEnd = false;

  while (rows.length < target) {
    const requestLimit = Math.min(SERVER_PAGE_LIMIT, target - rows.length);
    const page = await listHistory(requestLimit, cursor);
    rows = mergeHistoryRows(rows, page).slice(0, target);

    if (page.length < requestLimit) {
      reachedEnd = true;
      break;
    }

    const nextCursor = page[page.length - 1]?.timestamp_ms;
    if (nextCursor === undefined || nextCursor === cursor) {
      reachedEnd = true;
      break;
    }
    cursor = nextCursor;
  }

  return { rows, reachedEnd };
}

function scheduleRefresh() {
  if (listeners.size === 0) return;
  if (refreshTimer) clearTimeout(refreshTimer);
  refreshTimer = setTimeout(() => {
    refreshTimer = undefined;
    void refreshHistoryCache({ limit: EVENT_REFRESH_LIMIT });
  }, EVENT_REFRESH_DELAY_MS);
}

function ensureEvents() {
  if (eventsInstalled) return;
  eventsInstalled = true;
  onVoiceDone(() => {
    invalidationVersion += 1;
    stale = true;
    notify();
    scheduleRefresh();
  });
}

export function getHistoryCacheSnapshot(): HistoryCacheSnapshot {
  return { recordings, refreshing, stale, updatedAt };
}

export function subscribeHistoryCache(listener: Listener): () => void {
  ensureEvents();
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0 && refreshTimer) {
      clearTimeout(refreshTimer);
      refreshTimer = undefined;
    }
  };
}

export async function refreshHistoryCache(
  { limit = EVENT_REFRESH_LIMIT, force = false }: { limit?: number; force?: boolean } = {},
): Promise<Recording[]> {
  ensureEvents();
  if (!force && isFresh() && hasCoverage(limit)) return recordings!.slice(0, limit);

  if (inFlight) {
    await inFlight;
    if (hasCoverage(limit)) return recordings!.slice(0, limit);
  }

  refreshing = true;
  notify();
  const requestLimit = Math.max(1, limit);
  const requestVersion = invalidationVersion;
  inFlight = fetchHistoryWindow(requestLimit);
  try {
    const { rows: latest, reachedEnd } = await inFlight;
    const cached = recordings ?? [];
    if (reachedEnd) {
      // This response is the complete authoritative history. Replacing the
      // cache removes rows deleted since the previous refresh.
      recordings = latest;
    } else {
      // Replace the refreshed prefix while retaining older pages already
      // loaded by History. Missing rows inside the prefix were deleted.
      const oldestRefreshed = latest[latest.length - 1]?.timestamp_ms;
      const olderCached = oldestRefreshed === undefined
        ? []
        : cached.filter((recording) => recording.timestamp_ms < oldestRefreshed);
      recordings = mergeHistoryRows(olderCached, latest);
    }
    exhausted = reachedEnd;
    updatedAt = Date.now();
    stale = requestVersion !== invalidationVersion;
    if (stale) scheduleRefresh();
    return recordings.slice(0, limit);
  } finally {
    refreshing = false;
    inFlight = null;
    notify();
  }
}

export async function loadCachedHistoryPage(limit: number, before?: number): Promise<Recording[]> {
  if (before === undefined || before === null) return refreshHistoryCache({ limit });

  const localPage = (recordings ?? []).filter((recording) => recording.timestamp_ms < before).slice(0, limit);
  if (localPage.length >= limit || exhausted) return localPage;

  const { rows: page, reachedEnd } = await fetchHistoryWindow(limit, before);
  const result = mergeHistoryRows(localPage, page).slice(0, limit);
  recordings = mergeHistoryRows(recordings ?? [], page);
  exhausted = reachedEnd;
  notify();
  return result;
}

/** Revalidate after a local destructive history mutation. */
export function invalidateHistoryCache() {
  invalidationVersion += 1;
  stale = true;
  notify();
  scheduleRefresh();
}
