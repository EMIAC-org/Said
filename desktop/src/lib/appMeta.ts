import { useEffect, useState } from "react";
import { getAppIdentity } from "@/lib/invoke";

/** Resolved display metadata for a `target_app` key (bundle-id / exe path). */
export interface ResolvedAppMeta {
  name: string | null;
  icon: string | null; // data:image/png;base64,… or null
}

const EMPTY: ResolvedAppMeta = { name: null, icon: null };

// Module-level cache + in-flight dedup so many rows for the same app trigger
// only one backend round-trip.
const cache = new Map<string, ResolvedAppMeta>();
const inflight = new Map<string, Promise<ResolvedAppMeta>>();

/** Best-effort human label from a raw app key when the OS can't resolve one. */
export function prettyAppName(appKey: string): string {
  const base = appKey.split("/").pop() ?? appKey;
  const last = base.replace(/\.exe$/i, "").split(".").pop() ?? base;
  return last.charAt(0).toUpperCase() + last.slice(1);
}

function resolve(appKey: string): Promise<ResolvedAppMeta> {
  const hit = cache.get(appKey);
  if (hit) return Promise.resolve(hit);
  const pending = inflight.get(appKey);
  if (pending) return pending;

  const p = getAppIdentity(appKey)
    .then((id): ResolvedAppMeta => ({ name: id?.name ?? null, icon: id?.icon ?? null }))
    .catch((): ResolvedAppMeta => EMPTY)
    .then((meta) => {
      cache.set(appKey, meta);
      inflight.delete(appKey);
      return meta;
    });
  inflight.set(appKey, p);
  return p;
}

/**
 * Resolve an app key to its icon + display name, cached across the app.
 * Returns `{ name: null, icon: null }` until the backend responds (or for a
 * missing key), so callers can fall back to their own label.
 */
export function useAppMeta(appKey: string | null | undefined): ResolvedAppMeta {
  const [meta, setMeta] = useState<ResolvedAppMeta>(() =>
    appKey ? cache.get(appKey) ?? EMPTY : EMPTY,
  );

  useEffect(() => {
    if (!appKey || !appKey.trim()) {
      setMeta(EMPTY);
      return;
    }
    const cached = cache.get(appKey);
    if (cached) {
      setMeta(cached);
      return;
    }
    let alive = true;
    void resolve(appKey).then((m) => {
      if (alive) setMeta(m);
    });
    return () => {
      alive = false;
    };
  }, [appKey]);

  return meta;
}
