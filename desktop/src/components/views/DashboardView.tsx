import type { AppSnapshot } from "@/types";
import { EditorialDashboard } from "@/components/views/dashboards/EditorialDashboard";
import { SplitDashboard } from "@/components/views/dashboards/SplitDashboard";
import { useDashboardLayout } from "@/lib/useDashboardLayout";

interface DashboardViewProps {
  snapshot: AppSnapshot | null;
  onNavigate?: (view: string) => void;
  onDownloadSuccess?: (path: string) => void;
  refreshKey?: number;
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
