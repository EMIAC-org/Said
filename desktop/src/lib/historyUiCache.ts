import { listHistory, onVoiceDone } from "@/lib/invoke";
import type { Recording } from "@/types";

const FRESH_FOR_MS = 60_000;
const EVENT_REFRESH_DELAY_MS = 250;
const EVENT_REFRESH_LIMIT = 300;

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
let inFlight: Promise<Recording[]> | null = null;
let eventsInstalled = false;
let refreshTimer: ReturnType<typeof setTimeout> | undefined;
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

function mergeNewest(...sets: Recording[][]): Recording[] {
  const byId = new Map<string, Recording>();
  for (const set of sets) {
    for (const recording of set) byId.set(recording.id, recording);
  }
  return [...byId.values()].sort((a, b) => b.timestamp_ms - a.timestamp_ms);
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
  const requestLimit = limit;
  inFlight = listHistory(requestLimit);
  try {
    const latest = await inFlight;
    const previousLength = recordings?.length ?? 0;
    const retainedLength = Math.max(previousLength, latest.length);
    recordings = mergeNewest(latest, recordings ?? []).slice(0, retainedLength);
    if (latest.length < requestLimit) exhausted = true;
    updatedAt = Date.now();
    stale = false;
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

  const page = await listHistory(limit, before);
  recordings = mergeNewest(recordings ?? [], page);
  if (page.length < limit) exhausted = true;
  notify();
  return page;
}

/** Revalidate after a local destructive history mutation. */
export function invalidateHistoryCache() {
  stale = true;
  notify();
  scheduleRefresh();
}
