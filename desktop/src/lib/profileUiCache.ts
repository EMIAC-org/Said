import { listen } from "@tauri-apps/api/event";
import {
  getAppBuckets,
  getAppIcon,
  getAppIdentity,
  getProfileInsights,
  isTauriRuntime,
  onPendingEditsChanged,
  onVocabularyChanged,
  onVoiceDone,
  setAppBucket,
  setBucketLanguage,
  type AppBucketRow,
  type AppBuckets,
  type ProfileInsights,
} from "@/lib/invoke";

const FRESH_FOR_MS = 60_000;
const POST_DICTATION_REFRESH_DELAYS_MS = [1_500, 10_000];
const RETRY_DELAYS_MS = [2_000, 5_000, 10_000, 30_000];

type Unsubscribe = () => void;
type Listener = () => void;

export interface AppMeta {
  name: string;
  icon: string | null;
}

export interface LearningsCacheSnapshot {
  data: ProfileInsights | null | undefined;
  refreshing: boolean;
  stale: boolean;
  updatedAt: number | null;
}

export interface BucketsCacheSnapshot {
  buckets: string[] | null | undefined;
  apps: AppBucketRow[] | null | undefined;
  /** bucket_key -> output-language override (only buckets with one set). */
  bucketLanguages: Record<string, string>;
  meta: Record<string, AppMeta>;
  refreshing: boolean;
  stale: boolean;
  updatedAt: number | null;
}

let learningsData: ProfileInsights | null | undefined;
let learningsUpdatedAt: number | null = null;
let learningsStale = true;
let learningsRefreshing = false;
let learningsInFlight: Promise<ProfileInsights | null> | null = null;
let learningsRetryTimer: number | null = null;
let learningsFailureCount = 0;

let bucketsData: AppBuckets | null | undefined;
let bucketsUpdatedAt: number | null = null;
let bucketsStale = true;
let bucketsRefreshing = false;
let bucketsInFlight: Promise<AppBuckets | null> | null = null;
let bucketsRetryTimer: number | null = null;
let bucketsFailureCount = 0;
const appMetaCache: Record<string, AppMeta> = {};

const learningsListeners = new Set<Listener>();
const bucketsListeners = new Set<Listener>();
let eventListenersInstalled = false;

function now() {
  return Date.now();
}

function isFresh(updatedAt: number | null, stale: boolean) {
  return !!updatedAt && !stale && now() - updatedAt < FRESH_FOR_MS;
}

function notifyLearnings() {
  for (const listener of learningsListeners) listener();
}

function notifyBuckets() {
  for (const listener of bucketsListeners) listener();
}

export function getLearningsSnapshot(): LearningsCacheSnapshot {
  return {
    data: learningsData,
    refreshing: learningsRefreshing,
    stale: learningsStale,
    updatedAt: learningsUpdatedAt,
  };
}

export function getBucketsSnapshot(): BucketsCacheSnapshot {
  return {
    buckets: bucketsData?.buckets ?? (bucketsData === null ? null : undefined),
    apps: bucketsData?.apps ?? (bucketsData === null ? null : undefined),
    bucketLanguages: bucketsData?.bucket_languages ?? {},
    meta: { ...appMetaCache },
    refreshing: bucketsRefreshing,
    stale: bucketsStale,
    updatedAt: bucketsUpdatedAt,
  };
}

export function subscribeLearnings(listener: Listener): Unsubscribe {
  ensureProfileUiCacheEvents();
  learningsListeners.add(listener);
  return () => {
    learningsListeners.delete(listener);
    if (learningsListeners.size === 0) clearLearningsRetry();
  };
}

export function subscribeBuckets(listener: Listener): Unsubscribe {
  ensureProfileUiCacheEvents();
  bucketsListeners.add(listener);
  return () => {
    bucketsListeners.delete(listener);
    if (bucketsListeners.size === 0) clearBucketsRetry();
  };
}

export function refreshLearnings(options: { force?: boolean } = {}): Promise<ProfileInsights | null> {
  ensureProfileUiCacheEvents();
  if (!options.force && isFresh(learningsUpdatedAt, learningsStale)) {
    return Promise.resolve(learningsData ?? null);
  }
  if (learningsInFlight) return learningsInFlight;

  learningsRefreshing = true;
  notifyLearnings();
  learningsInFlight = getProfileInsights()
    .then((data) => {
      if (data) {
        learningsData = data;
        learningsUpdatedAt = now();
        learningsStale = false;
        learningsFailureCount = 0;
        clearLearningsRetry();
      } else {
        if (learningsData === undefined) learningsData = null;
        learningsStale = true;
        scheduleLearningsRetry();
      }
      return data;
    })
    .finally(() => {
      learningsRefreshing = false;
      learningsInFlight = null;
      notifyLearnings();
    });

  return learningsInFlight;
}

export function refreshBuckets(options: { force?: boolean } = {}): Promise<AppBuckets | null> {
  ensureProfileUiCacheEvents();
  if (!options.force && isFresh(bucketsUpdatedAt, bucketsStale)) {
    return Promise.resolve(bucketsData ?? null);
  }
  if (bucketsInFlight) return bucketsInFlight;

  bucketsRefreshing = true;
  notifyBuckets();
  bucketsInFlight = getAppBuckets()
    .then(async (data) => {
      if (data) {
        bucketsData = data;
        bucketsUpdatedAt = now();
        bucketsStale = false;
        bucketsFailureCount = 0;
        clearBucketsRetry();
        notifyBuckets();
        await resolveMissingAppMeta(data.apps);
      } else {
        if (bucketsData === undefined) bucketsData = null;
        bucketsStale = true;
        scheduleBucketsRetry();
      }
      return data;
    })
    .finally(() => {
      bucketsRefreshing = false;
      bucketsInFlight = null;
      notifyBuckets();
    });

  return bucketsInFlight;
}

export async function moveAppBucketCached(appKey: string, bucketKey: string): Promise<void> {
  const previous = bucketsData ? { ...bucketsData, apps: [...bucketsData.apps] } : bucketsData;
  if (bucketsData) {
    bucketsData = {
      ...bucketsData,
      apps: bucketsData.apps.map((app) =>
        app.app_key === appKey ? { ...app, bucket_key: bucketKey, source: "user" } : app,
      ),
    };
    bucketsStale = true;
    notifyBuckets();
  }

  try {
    await setAppBucket(appKey, bucketKey);
    markBucketsStale();
    markLearningsStale();
    await refreshBuckets({ force: true });
  } catch (err) {
    bucketsData = previous;
    bucketsStale = true;
    notifyBuckets();
    void refreshBuckets({ force: true });
    throw err;
  }
}

export async function setBucketLanguageCached(
  bucketKey: string,
  outputLanguage: string | null,
): Promise<void> {
  const previous = bucketsData ? { ...bucketsData } : bucketsData;
  if (bucketsData) {
    const next = { ...(bucketsData.bucket_languages ?? {}) };
    if (outputLanguage) next[bucketKey] = outputLanguage;
    else delete next[bucketKey];
    bucketsData = { ...bucketsData, bucket_languages: next };
    bucketsStale = true;
    notifyBuckets();
  }

  try {
    await setBucketLanguage(bucketKey, outputLanguage);
    markBucketsStale();
    await refreshBuckets({ force: true });
  } catch (err) {
    bucketsData = previous;
    bucketsStale = true;
    notifyBuckets();
    void refreshBuckets({ force: true });
    throw err;
  }
}

function markLearningsStale() {
  learningsStale = true;
  notifyLearnings();
}

function markBucketsStale() {
  bucketsStale = true;
  notifyBuckets();
}

function refreshObservedCaches() {
  if (learningsListeners.size > 0) void refreshLearnings({ force: true });
  if (bucketsListeners.size > 0) void refreshBuckets({ force: true });
}

function refreshStaleObservedCaches() {
  if (learningsListeners.size > 0 && (learningsStale || !learningsUpdatedAt)) {
    void refreshLearnings({ force: true });
  }
  if (bucketsListeners.size > 0 && (bucketsStale || !bucketsUpdatedAt)) {
    void refreshBuckets({ force: true });
  }
}

function markAllStaleAndRefreshSoon() {
  markLearningsStale();
  markBucketsStale();
  for (const delay of POST_DICTATION_REFRESH_DELAYS_MS) {
    window.setTimeout(refreshObservedCaches, delay);
  }
}

function clearAllCachedUiData() {
  learningsData = undefined;
  learningsUpdatedAt = null;
  learningsStale = true;
  learningsFailureCount = 0;
  clearLearningsRetry();
  bucketsData = undefined;
  bucketsUpdatedAt = null;
  bucketsStale = true;
  bucketsFailureCount = 0;
  clearBucketsRetry();
  for (const key of Object.keys(appMetaCache)) delete appMetaCache[key];
  notifyLearnings();
  notifyBuckets();
}

function scheduleLearningsRetry() {
  if (learningsRetryTimer !== null || learningsListeners.size === 0) return;
  const delay = RETRY_DELAYS_MS[Math.min(learningsFailureCount, RETRY_DELAYS_MS.length - 1)];
  learningsFailureCount += 1;
  learningsRetryTimer = window.setTimeout(() => {
    learningsRetryTimer = null;
    if (learningsListeners.size > 0 && learningsStale) {
      void refreshLearnings({ force: true });
    }
  }, delay);
}

function clearLearningsRetry() {
  if (learningsRetryTimer === null) return;
  window.clearTimeout(learningsRetryTimer);
  learningsRetryTimer = null;
}

function scheduleBucketsRetry() {
  if (bucketsRetryTimer !== null || bucketsListeners.size === 0) return;
  const delay = RETRY_DELAYS_MS[Math.min(bucketsFailureCount, RETRY_DELAYS_MS.length - 1)];
  bucketsFailureCount += 1;
  bucketsRetryTimer = window.setTimeout(() => {
    bucketsRetryTimer = null;
    if (bucketsListeners.size > 0 && bucketsStale) {
      void refreshBuckets({ force: true });
    }
  }, delay);
}

function clearBucketsRetry() {
  if (bucketsRetryTimer === null) return;
  window.clearTimeout(bucketsRetryTimer);
  bucketsRetryTimer = null;
}

async function resolveMissingAppMeta(apps: AppBucketRow[]) {
  const missing = apps.filter((app) => !appMetaCache[app.app_key]);
  if (missing.length === 0) return;

  await Promise.all(
    missing.map(async (app) => {
      const [icon, identity] = await Promise.all([
        getAppIcon(app.app_key),
        getAppIdentity(app.app_key),
      ]);
      appMetaCache[app.app_key] = {
        name: identity?.name ?? prettyKey(app.app_key),
        icon,
      };
    }),
  );
  notifyBuckets();
}

function prettyKey(appKey: string): string {
  const base = appKey.split("/").pop() ?? appKey;
  const last = base.replace(/\.exe$/i, "").split(".").pop() ?? base;
  return last.charAt(0).toUpperCase() + last.slice(1);
}

function ensureProfileUiCacheEvents() {
  if (eventListenersInstalled || !isTauriRuntime()) return;
  eventListenersInstalled = true;

  onVoiceDone(() => {
    markAllStaleAndRefreshSoon();
  });
  onVocabularyChanged(() => {
    markAllStaleAndRefreshSoon();
  });
  onPendingEditsChanged(() => {
    markAllStaleAndRefreshSoon();
  });
  listen("learning_saved", () => {
    markAllStaleAndRefreshSoon();
  }).catch(() => {});
  listen("retrain-status", () => {
    markAllStaleAndRefreshSoon();
  }).catch(() => {});
  window.addEventListener("airnote-enterprise-connection-changed", () => {
    clearAllCachedUiData();
    refreshObservedCaches();
  });
  window.addEventListener("focus", refreshStaleObservedCaches);
  window.addEventListener("online", refreshStaleObservedCaches);
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "visible") refreshStaleObservedCaches();
  });
}
