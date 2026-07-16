import { useEffect, useState } from "react";
import { Sparkles } from "lucide-react";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Skeleton } from "@/components/ui/skeleton";
import {
  getLearningsSnapshot,
  refreshLearnings,
  subscribeLearnings,
  type LearningsCacheSnapshot,
} from "@/lib/profileUiCache";

const BUCKET_LABELS: Record<string, string> = {
  coding: "Coding",
  messaging: "Messaging",
  work_tracker: "Work & Tasks",
  formal_writing: "Formal Writing",
  default: "General",
};

function relativeTime(iso: string | null): string {
  if (!iso) return "";
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return "";
  const mins = Math.max(0, Math.round((Date.now() - then) / 60000));
  if (mins < 1) return "just now";
  if (mins < 60) return `${mins}m ago`;
  const hrs = Math.round(mins / 60);
  if (hrs < 24) return `${hrs}h ago`;
  return `${Math.round(hrs / 24)}d ago`;
}

// ── Loading skeleton — mirrors the knowledge-base panel ─────────────────────
function LearningsSkeleton() {
  return (
    <div className="panel p-5">
      <div className="flex items-center justify-between mb-4">
        <Skeleton className="h-3.5 w-40" />
        <Skeleton className="h-3 w-16" />
      </div>
      <div className="space-y-2 mb-5">
        <Skeleton className="h-3 w-full" />
        <Skeleton className="h-3 w-11/12" />
        <Skeleton className="h-3 w-3/4" />
      </div>
      <div className="flex flex-wrap gap-1.5 mb-6">
        {[16, 20, 12, 24, 14].map((w, i) => (
          <Skeleton key={i} className="h-5 rounded-md" style={{ width: w * 4 }} />
        ))}
      </div>
      <Skeleton className="h-2.5 w-32 mb-3" />
      <div className="space-y-3">
        {[0, 1].map((i) => (
          <div key={i} className="rounded-lg p-3" style={{ background: "hsl(var(--surface-4))" }}>
            <Skeleton className="h-3 w-28 mb-2.5" style={{ background: "hsl(var(--surface-3))" }} />
            <div className="space-y-1.5">
              <Skeleton className="h-2.5 w-full" style={{ background: "hsl(var(--surface-3))" }} />
              <Skeleton className="h-2.5 w-5/6" style={{ background: "hsl(var(--surface-3))" }} />
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}

/**
 * Learnings — the knowledge base AirNote keeps overwriting about you:
 * the global background/domains/vocab, plus the learned per-context style.
 */
export function LearningsView() {
  const [snapshot, setSnapshot] = useState<LearningsCacheSnapshot>(() => getLearningsSnapshot());

  useEffect(() => {
    const unsubscribe = subscribeLearnings(() => setSnapshot(getLearningsSnapshot()));
    void refreshLearnings();
    return unsubscribe;
  }, []);

  const data = snapshot.data;
  const run_stats = data?.run_stats;
  const knowledge = data?.knowledge;
  const buckets = data?.buckets ?? [];
  const hasKnowledge =
    !!knowledge?.background ||
    (knowledge?.domains.length ?? 0) > 0 ||
    (knowledge?.focus_areas.length ?? 0) > 0;
  const learned = (run_stats?.run_count ?? 0) > 0 || buckets.length > 0 || hasKnowledge;
  const ago = relativeTime(run_stats?.last_run_at ?? null);
  const tags = [...(knowledge?.domains ?? []), ...(knowledge?.focus_areas ?? [])];

  return (
    <ScrollArea className="h-full">
      <div className="p-5 pb-12 mx-auto overflow-hidden" style={{ maxWidth: "min(900px, 100%)" }}>
        {/* Header */}
        <div className="mb-7">
          <h1 className="text-[24px] font-bold tracking-tight text-foreground leading-tight">
            Learnings
          </h1>
          <p className="text-[12.5px] text-muted-foreground mt-1 flex items-center gap-2">
            <span
              className="inline-block w-1.5 h-1.5 rounded-full"
              style={{
                background: "hsl(var(--accent-violet))",
                boxShadow: "0 0 8px hsl(var(--accent-violet) / 0.5)",
              }}
            />
            What AirNote knows about you
          </p>
        </div>

        {/* Signed out / offline / cloud runtime off */}
        {data === null && (
          <div className="panel p-5">
            <p className="text-[12px] text-muted-foreground">
              Couldn’t load your learnings. Make sure you’re signed in to the cloud
              workspace and running the latest build, then reopen this page.
            </p>
          </div>
        )}

        {/* Loading */}
        {data === undefined && <LearningsSkeleton />}

        {/* Warming up — endpoint responded but nothing learned yet */}
        {data && !learned && (
          <div className="panel p-5">
            <div className="flex items-baseline justify-between mb-2">
              <h2 className="text-[14px] font-semibold text-foreground flex items-center gap-2">
                <Sparkles size={14} style={{ color: "hsl(var(--accent-violet))" }} />
                Still learning
              </h2>
              <span className="section-label">warming up</span>
            </div>
            <p className="text-[12px] text-muted-foreground">
              Keep dictating — after a handful of dictations AirNote builds a per-app profile
              and a knowledge base about you, and it shows up here.
            </p>
          </div>
        )}

        {data && learned && run_stats && knowledge && (
          <div className="panel p-5">
            <div className="flex items-baseline justify-between mb-2">
              <h2 className="text-[14px] font-semibold text-foreground flex items-center gap-2">
                <Sparkles size={14} style={{ color: "hsl(var(--accent-violet))" }} />
                Knowledge base
              </h2>
              <span className="section-label">
                {run_stats.run_count} run{run_stats.run_count === 1 ? "" : "s"}
                {snapshot.refreshing ? " · refreshing" : ""}
              </span>
            </div>
            <p className="text-[12px] text-muted-foreground mb-5">
              AirNote has studied your dictation {run_stats.run_count} time
              {run_stats.run_count === 1 ? "" : "s"}
              {ago ? ` · last updated ${ago}` : ""}
              {run_stats.skipped_count > 0
                ? ` · ${run_stats.skipped_count} skipped (nothing new)`
                : ""}
            </p>

            {hasKnowledge && (
              <div className="mb-5">
                {knowledge.background && (
                  <p className="text-[13px] text-foreground leading-relaxed mb-3">
                    {knowledge.background}
                  </p>
                )}
                {tags.length > 0 && (
                  <div className="flex flex-wrap gap-1.5">
                    {tags.map((t, i) => (
                      <span
                        key={`${t}-${i}`}
                        className="text-[11px] px-2 py-0.5 rounded-md"
                        style={{
                          background: "hsl(var(--surface-4))",
                          color: "hsl(var(--muted-foreground))",
                        }}
                      >
                        {t}
                      </span>
                    ))}
                  </div>
                )}
              </div>
            )}

            {buckets.length > 0 && (
              <div>
                <div className="section-label mb-2">Your style by context</div>
                <div className="space-y-3">
                  {buckets.map((b) => {
                    const lines = [...b.style, ...b.speech_patterns];
                    if (lines.length === 0) return null;
                    return (
                      <div
                        key={b.bucket_key}
                        className="rounded-lg p-3"
                        style={{ background: "hsl(var(--surface-4))" }}
                      >
                        <div className="text-[13px] font-medium text-foreground mb-1.5">
                          {BUCKET_LABELS[b.bucket_key] ?? b.bucket_key}
                        </div>
                        <ul className="space-y-1">
                          {lines.map((l, i) => (
                            <li key={i} className="text-[12px] text-muted-foreground flex gap-2">
                              <span style={{ color: "hsl(var(--accent-violet))" }}>·</span>
                              <span>{l}</span>
                            </li>
                          ))}
                        </ul>
                      </div>
                    );
                  })}
                </div>
              </div>
            )}
          </div>
        )}
      </div>
    </ScrollArea>
  );
}
