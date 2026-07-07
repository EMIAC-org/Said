import type { AppSnapshot, PendingEdit } from "@/types";
import { EditorialDashboard } from "@/components/views/dashboards/EditorialDashboard";
import { SplitDashboard } from "@/components/views/dashboards/SplitDashboard";
import { useDashboardLayout } from "@/lib/useDashboardLayout";

// ── Props ──────────────────────────────────────────────────────────────────────
// Kept identical to the previous DashboardView so App.tsx doesn't change.

interface DashboardViewProps {
  snapshot:        AppSnapshot | null;
  busy:            boolean;
  onToggle:        () => void;
  onAccessibility: () => void;
  onNavigate?:     (view: string) => void;
  pendingEdits?:   PendingEdit[];
  onResolvePending?: (id: string, action: "approve" | "skip") => void;
  onDownloadSuccess?: (path: string) => void;
  refreshKey?:     number;
}

// ── View ───────────────────────────────────────────────────────────────────────

export function DashboardView({
  snapshot,
  onNavigate,
  onDownloadSuccess,
  refreshKey,
}: DashboardViewProps) {
  const { layout } = useDashboardLayout();

  if (layout === "split") {
    return (
      <SplitDashboard
        snapshot={snapshot}
        onNavigate={onNavigate}
        onDownloadSuccess={onDownloadSuccess}
        refreshKey={refreshKey}
      />
    );
  }

  return (
    <EditorialDashboard
      snapshot={snapshot}
    />
  );
}
