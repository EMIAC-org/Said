import {
  listVocabulary,
  listVocabularyAliases,
  onVocabularyChanged,
  type VocabAlias,
  type VocabRow,
} from "@/lib/invoke";

const FRESH_FOR_MS = 60_000;
const EVENT_REFRESH_DELAY_MS = 100;

type Listener = () => void;

export interface VocabularyCacheSnapshot {
  terms: VocabRow[] | undefined;
  aliases: VocabAlias[] | undefined;
  total: number;
  refreshing: boolean;
  stale: boolean;
  updatedAt: number | null;
}

let terms: VocabRow[] | undefined;
let aliases: VocabAlias[] | undefined;
let total = 0;
let updatedAt: number | null = null;
let stale = true;
let refreshing = false;
let inFlight: Promise<VocabularyCacheSnapshot> | null = null;
let eventsInstalled = false;
let refreshTimer: ReturnType<typeof setTimeout> | undefined;
const listeners = new Set<Listener>();

function isFresh() {
  return !!updatedAt && !stale && Date.now() - updatedAt < FRESH_FOR_MS;
}

function notify() {
  for (const listener of listeners) listener();
}

function snapshot(): VocabularyCacheSnapshot {
  return { terms, aliases, total, refreshing, stale, updatedAt };
}

function scheduleRefresh() {
  if (listeners.size === 0) return;
  if (refreshTimer) clearTimeout(refreshTimer);
  refreshTimer = setTimeout(() => {
    refreshTimer = undefined;
    void refreshVocabularyCache({ force: true });
  }, EVENT_REFRESH_DELAY_MS);
}

function ensureEvents() {
  if (eventsInstalled) return;
  eventsInstalled = true;
  onVocabularyChanged(() => {
    stale = true;
    notify();
    scheduleRefresh();
  });
}

export function getVocabularyCacheSnapshot(): VocabularyCacheSnapshot {
  return snapshot();
}

export function subscribeVocabularyCache(listener: Listener): () => void {
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

export async function refreshVocabularyCache(
  { force = false }: { force?: boolean } = {},
): Promise<VocabularyCacheSnapshot> {
  ensureEvents();
  if (!force && isFresh() && terms && aliases) return snapshot();
  if (inFlight) return inFlight;

  refreshing = true;
  notify();
  inFlight = Promise.all([listVocabulary(), listVocabularyAliases()])
    .then(([vocabulary, aliasRows]) => {
      terms = vocabulary.terms;
      aliases = aliasRows.aliases;
      total = vocabulary.total;
      updatedAt = Date.now();
      stale = false;
      return snapshot();
    })
    .finally(() => {
      refreshing = false;
      inFlight = null;
      notify();
    });
  return inFlight;
}

/** Revalidate after a local vocabulary mutation that may not emit immediately. */
export function invalidateVocabularyCache() {
  stale = true;
  notify();
  scheduleRefresh();
}
