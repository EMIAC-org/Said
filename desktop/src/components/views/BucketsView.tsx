import { useEffect, useMemo, useState } from "react";
import { MoreHorizontal } from "lucide-react";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Skeleton } from "@/components/ui/skeleton";
import {
  type AppBucketRow,
} from "@/lib/invoke";
import {
  getBucketsSnapshot,
  moveAppBucketCached,
  refreshBuckets,
  subscribeBuckets,
  type BucketsCacheSnapshot,
} from "@/lib/profileUiCache";

const BUCKET_LABELS: Record<string, string> = {
  coding: "Coding",
  messaging: "Messaging",
  work_tracker: "Work & Tasks",
  formal_writing: "Formal Writing",
  default: "General",
};

function bucketLabel(key: string): string {
  return BUCKET_LABELS[key] ?? key.replace(/_/g, " ");
}

function prettyKey(appKey: string): string {
  // com.microsoft.vscode -> Vscode ; /path/Cursor.exe -> Cursor
  const base = appKey.split("/").pop() ?? appKey;
  const last = base.replace(/\.exe$/i, "").split(".").pop() ?? base;
  return last.charAt(0).toUpperCase() + last.slice(1);
}

// ── Loading skeleton — mirrors the kanban of bucket columns ─────────────────
function BucketsSkeleton() {
  return (
    <div className="flex gap-4 overflow-hidden pb-2">
      {[0, 1, 2, 3].map((col) => (
        <div
          key={col}
          className="panel p-3 flex-shrink-0 flex flex-col"
          style={{ width: 272, minHeight: 200 }}
        >
          <div className="flex items-center justify-between mb-3 px-1">
            <Skeleton className="h-3.5 w-24" />
            <Skeleton className="h-4 w-6 rounded-md" />
          </div>
          <div className="space-y-2 flex-1">
            {Array.from({ length: 3 - (col % 2) }).map((_, i) => (
              <div
                key={i}
                className="rounded-xl p-3 flex items-center gap-3"
                style={{ background: "hsl(var(--surface-4))" }}
              >
                <Skeleton className="w-7 h-7 rounded-lg flex-shrink-0" style={{ background: "hsl(var(--surface-3))" }} />
                <Skeleton className="h-3" style={{ width: `${60 - i * 12}%`, background: "hsl(var(--surface-3))" }} />
              </div>
            ))}
          </div>
        </div>
      ))}
    </div>
  );
}

/**
 * Buckets — a kanban of every app you dictate into, grouped by its bucket.
 * The AI classifies unknown apps; if it's wrong, use a card's ⋯ menu (or drag it)
 * to re-file it. That writes a user override which wins over the AI's guess.
 */
export function BucketsView() {
  const [snapshot, setSnapshot] = useState<BucketsCacheSnapshot>(() => getBucketsSnapshot());
  const [dragging, setDragging] = useState<string | null>(null);
  const [overCol, setOverCol] = useState<string | null>(null);
  const [menuFor, setMenuFor] = useState<string | null>(null);

  useEffect(() => {
    const unsubscribe = subscribeBuckets(() => setSnapshot(getBucketsSnapshot()));
    void refreshBuckets();
    return unsubscribe;
  }, []);

  const buckets = snapshot.buckets;
  const apps = snapshot.apps;
  const meta = snapshot.meta;

  const byBucket = useMemo(() => {
    const m: Record<string, AppBucketRow[]> = {};
    for (const b of buckets ?? []) m[b] = [];
    for (const a of apps ?? []) (m[a.bucket_key] ??= []).push(a);
    for (const k of Object.keys(m)) m[k].sort((x, y) => y.count - x.count);
    return m;
  }, [buckets, apps]);

  async function move(appKey: string, toBucket: string) {
    setMenuFor(null);
    try {
      await moveAppBucketCached(appKey, toBucket);
    } catch {
      // Shared cache rolls back and revalidates. Keep the UI stable.
    }
  }

  return (
    <ScrollArea className="h-full">
      <div className="p-5 pb-12">
        {/* Header */}
        <div className="mb-6">
          <h1 className="text-[24px] font-bold tracking-tight text-foreground leading-tight">
            Buckets
          </h1>
          <p className="text-[12.5px] text-muted-foreground mt-1 flex items-center gap-2">
            <span
              className="inline-block w-1.5 h-1.5 rounded-full"
              style={{
                background: "hsl(var(--accent-violet))",
                boxShadow: "0 0 8px hsl(var(--accent-violet) / 0.5)",
              }}
            />
            How your apps are grouped — use a card’s ⋯ menu (or drag it) to re-file
            {snapshot.refreshing && apps !== undefined ? " · refreshing" : ""}
          </p>
        </div>

        {apps === null && (
          <div className="panel p-5">
            <p className="text-[12px] text-muted-foreground">
              Couldn’t load your app buckets. Make sure you’re signed in to the cloud
              workspace and running the latest build, then reopen this page.
            </p>
          </div>
        )}
        {apps === undefined && <BucketsSkeleton />}
        {apps && apps.length === 0 && (
          <div className="panel p-5">
            <p className="text-[12px] text-muted-foreground">
              Dictate into your apps and they’ll show up here, grouped by bucket.
            </p>
          </div>
        )}

        {apps && apps.length > 0 && buckets && (
          <div className="flex gap-4 overflow-x-auto pb-2" style={{ scrollbarWidth: "thin" }}>
            {buckets.map((bk) => {
              const cards = byBucket[bk] ?? [];
              const isOver = overCol === bk;
              return (
                <div
                  key={bk}
                  onDragOver={(e) => {
                    e.preventDefault();
                    setOverCol(bk);
                  }}
                  onDragLeave={() => setOverCol((c) => (c === bk ? null : c))}
                  onDrop={(e) => {
                    e.preventDefault();
                    setOverCol(null);
                    const dropped = dragging || e.dataTransfer.getData("text/plain");
                    if (dropped) void move(dropped, bk);
                    setDragging(null);
                  }}
                  className="panel p-3 flex-shrink-0 flex flex-col"
                  style={{
                    width: 272,
                    minHeight: 200,
                    outline: isOver
                      ? "1.5px solid hsl(var(--accent-violet))"
                      : "1.5px solid transparent",
                    transition: "outline-color 120ms",
                  }}
                >
                  <div className="flex items-center justify-between mb-3 px-1">
                    <h2 className="text-[14px] font-semibold text-foreground">
                      {bucketLabel(bk)}
                    </h2>
                    <span
                      className="text-[11px] font-semibold tabular-nums px-1.5 py-0.5 rounded-md"
                      style={{
                        background: "hsl(var(--surface-4))",
                        color: "hsl(var(--muted-foreground))",
                      }}
                    >
                      {cards.length}
                    </span>
                  </div>

                  <div className="space-y-2 flex-1">
                    {cards.map((a) => {
                      const m = meta[a.app_key];
                      const isUser = a.source === "user";
                      return (
                        <div
                          key={a.app_key}
                          draggable
                          onDragStart={(e) => {
                            e.dataTransfer.setData("text/plain", a.app_key);
                            e.dataTransfer.effectAllowed = "move";
                            setDragging(a.app_key);
                          }}
                          onDragEnd={() => {
                            setDragging(null);
                            setOverCol(null);
                          }}
                          className="relative rounded-xl p-3 flex items-center gap-3 cursor-grab active:cursor-grabbing"
                          style={{
                            background: "hsl(var(--surface-4))",
                            opacity: dragging === a.app_key ? 0.4 : 1,
                          }}
                        >
                          {m?.icon ? (
                            <img
                              src={m.icon}
                              alt=""
                              className="w-8 h-8 rounded-lg flex-shrink-0"
                            />
                          ) : (
                            <div
                              className="w-8 h-8 rounded-lg flex-shrink-0"
                              style={{ background: "hsl(var(--surface-3))" }}
                            />
                          )}

                          <div className="flex-1 min-w-0">
                            <div className="text-[13px] font-medium text-foreground truncate">
                              {m?.name ?? prettyKey(a.app_key)}
                            </div>
                            <div className="text-[11px] text-muted-foreground flex items-center gap-1.5 mt-0.5 whitespace-nowrap">
                              <span className="tabular-nums">
                                {a.count} dictation{a.count === 1 ? "" : "s"}
                              </span>
                              <span>·</span>
                              <span
                                className="px-1.5 py-px rounded"
                                style={{
                                  background: isUser
                                    ? "hsl(var(--accent-violet) / 0.18)"
                                    : "hsl(var(--surface-3))",
                                  color: isUser
                                    ? "hsl(var(--accent-violet))"
                                    : "hsl(var(--muted-foreground))",
                                }}
                              >
                                {isUser ? "you" : "AI"}
                              </span>
                            </div>
                          </div>

                          {/* ⋯ move menu (reliable — drag can be flaky in the webview) */}
                          <button
                            title="Move to another bucket"
                            onClick={(e) => {
                              e.stopPropagation();
                              setMenuFor((cur) => (cur === a.app_key ? null : a.app_key));
                            }}
                            className="flex-shrink-0 w-7 h-7 rounded-lg flex items-center justify-center text-muted-foreground hover:text-foreground hover:bg-white/5 transition-colors"
                          >
                            <MoreHorizontal size={15} />
                          </button>

                          {menuFor === a.app_key && (
                            <div
                              className="absolute z-20 right-2 top-12 rounded-xl py-1 shadow-xl overflow-hidden"
                              style={{
                                background: "hsl(var(--surface-2))",
                                border: "1px solid hsl(var(--surface-4))",
                                minWidth: 168,
                              }}
                            >
                              {buckets
                                .filter((opt) => opt !== a.bucket_key)
                                .map((opt) => (
                                  <button
                                    key={opt}
                                    onClick={(e) => {
                                      e.stopPropagation();
                                      void move(a.app_key, opt);
                                    }}
                                    className="w-full text-left px-3 py-1.5 text-[12.5px] text-foreground hover:bg-white/5 transition-colors"
                                  >
                                    Move to {bucketLabel(opt)}
                                  </button>
                                ))}
                            </div>
                          )}
                        </div>
                      );
                    })}
                    {cards.length === 0 && (
                      <div className="text-[11px] text-muted-foreground/60 px-1 py-6 text-center">
                        Drop apps here
                      </div>
                    )}
                  </div>
                </div>
              );
            })}
          </div>
        )}
      </div>

      {/* Click-away layer to dismiss the open menu. */}
      {menuFor && (
        <div className="fixed inset-0 z-10" onClick={() => setMenuFor(null)} />
      )}
    </ScrollArea>
  );
}
