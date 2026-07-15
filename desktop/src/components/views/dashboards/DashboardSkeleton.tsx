import { ScrollArea } from "@/components/ui/scroll-area";
import { Skeleton } from "@/components/ui/skeleton";

const panelShadow = "inset 0 0 0 1px hsl(var(--glass-stroke-strong))";

/** Loading placeholder for the Split (two-column) dashboard layout. */
export function SplitDashboardSkeleton() {
  return (
    <div className="h-full overflow-hidden" style={{ padding: 16 }}>
      <div className="grid h-full" style={{ gridTemplateColumns: "minmax(0,1fr) minmax(0,1.3fr)", gap: 12 }}>
        {/* Left column — pace card, mini tiles, apps */}
        <div className="flex flex-col gap-3 min-w-0 overflow-hidden">
          <div className="rounded-xl p-4" style={{ background: "hsl(var(--surface-3))", boxShadow: panelShadow }}>
            <Skeleton className="h-2.5 w-1/2" />
            <Skeleton className="h-8 w-24 mt-3" />
            <Skeleton className="h-2.5 w-2/5 mt-2" />
            <div className="mt-3 flex items-end gap-[3px]" style={{ height: 36 }}>
              {Array.from({ length: 10 }).map((_, i) => (
                <Skeleton key={i} className="flex-1 rounded-[2px]" style={{ height: 8 + ((i * 7) % 28) }} />
              ))}
            </div>
          </div>
          <div className="grid gap-3" style={{ gridTemplateColumns: "1fr 1fr" }}>
            {[0, 1].map((i) => (
              <div key={i} className="rounded-xl p-4" style={{ background: "hsl(var(--surface-3))", boxShadow: panelShadow }}>
                <Skeleton className="h-2.5 w-2/3" />
                <Skeleton className="h-6 w-16 mt-3" />
                <Skeleton className="h-2 w-1/2 mt-2" />
              </div>
            ))}
          </div>
          <div className="rounded-xl p-4 flex-1" style={{ background: "hsl(var(--surface-3))", boxShadow: panelShadow }}>
            <Skeleton className="h-2.5 w-1/3" />
            <div className="mt-4 space-y-3">
              {[0, 1, 2, 3].map((i) => (
                <div key={i} className="flex items-center gap-3">
                  <Skeleton className="w-7 h-7 rounded-lg" />
                  <Skeleton className="h-3 flex-1" style={{ maxWidth: `${70 - i * 10}%` }} />
                  <Skeleton className="h-3 w-8" />
                </div>
              ))}
            </div>
          </div>
        </div>

        {/* Right column — timeline */}
        <div className="rounded-xl p-4 overflow-hidden" style={{ background: "hsl(var(--surface-3))", boxShadow: panelShadow }}>
          <Skeleton className="h-3 w-1/4" />
          <div className="mt-4 space-y-3">
            {Array.from({ length: 7 }).map((_, i) => (
              <div key={i} className="flex gap-3 rounded-xl p-3" style={{ boxShadow: "inset 0 0 0 1px hsl(var(--border))" }}>
                <Skeleton className="w-8 h-8 rounded-lg" />
                <div className="flex-1 space-y-2 pt-0.5">
                  <Skeleton className="h-3" style={{ width: `${80 - (i % 4) * 12}%` }} />
                  <Skeleton className="h-2.5 w-1/3" />
                </div>
              </div>
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}

/** Loading placeholder for the Editorial (single-column) dashboard layout. */
export function EditorialDashboardSkeleton() {
  return (
    <ScrollArea className="h-full">
      <div className="mx-auto" style={{ maxWidth: "min(720px, 100%)", padding: "24px 28px 40px" }}>
        {/* Hero */}
        <div className="mb-7">
          <Skeleton className="h-2.5 w-32 mb-3" />
          <Skeleton className="h-8 w-3/4" />
          <Skeleton className="h-8 w-1/2 mt-2" />
        </div>

        {/* Stat row */}
        <div className="grid gap-3 mb-7" style={{ gridTemplateColumns: "repeat(3, 1fr)" }}>
          {[0, 1, 2].map((i) => (
            <div key={i} className="rounded-xl p-4" style={{ background: "hsl(var(--surface-3))", boxShadow: panelShadow }}>
              <Skeleton className="h-6 w-14" />
              <Skeleton className="h-2.5 w-2/3 mt-2.5" />
            </div>
          ))}
        </div>

        {/* Bar chart */}
        <div className="rounded-xl p-4 mb-7" style={{ background: "hsl(var(--surface-3))", boxShadow: panelShadow }}>
          <Skeleton className="h-2.5 w-1/4 mb-4" />
          <div className="flex items-end gap-1.5" style={{ height: 88 }}>
            {Array.from({ length: 14 }).map((_, i) => (
              <Skeleton key={i} className="flex-1" style={{ height: 20 + ((i * 13) % 60) }} />
            ))}
          </div>
        </div>

        {/* Today's recordings */}
        <Skeleton className="h-2.5 w-1/5 mb-4" />
        <div className="space-y-3">
          {[0, 1, 2, 3].map((i) => (
            <div key={i} className="flex gap-3 rounded-xl px-4 py-3.5" style={{ boxShadow: "inset 0 0 0 1px hsl(var(--border))" }}>
              <Skeleton className="w-8 h-8 rounded-lg" />
              <div className="flex-1 space-y-2 pt-1">
                <Skeleton className="h-3" style={{ width: `${75 - i * 9}%` }} />
                <Skeleton className="h-2.5 w-2/5" />
              </div>
            </div>
          ))}
        </div>
      </div>
    </ScrollArea>
  );
}
