import type { AppSnapshot, PendingEdit } from "@/types";
import { useDashboardLayout } from "@/lib/useDashboardLayout";
import { EditorialDashboard } from "@/components/views/dashboards/EditorialDashboard";
import { SplitDashboard }     from "@/components/views/dashboards/SplitDashboard";

// ── Props ──────────────────────────────────────────────────────────────────────
// Kept identical to the previous DashboardView so App.tsx doesn't change.

interface DashboardViewProps {
  snapshot:        AppSnapshot | null;
  busy:            boolean;
  onToggle:        () => void;
  onAccessibility: () => void;
  onNavigate?:     (view: string) => void;
  statusPhase?:    string;
  liveText?:       string;
  pendingEdits?:   PendingEdit[];
  onResolvePending?: (id: string, action: "approve" | "skip") => void;
  onDownloadSuccess?: (path: string) => void;
  refreshKey?:     number;
}

// ── View ───────────────────────────────────────────────────────────────────────

export function DashboardView({
  snapshot,
  statusPhase       = "",
  liveText          = "",
  onNavigate,
  onDownloadSuccess,
  refreshKey        = 0,
}: DashboardViewProps) {
  const { layout } = useDashboardLayout();

  if (layout === "editorial") {
    return (
      <EditorialDashboard
        snapshot={snapshot}
        statusPhase={statusPhase}
        liveText={liveText}
      />
    );
  }

  return (
    <SplitDashboard
      snapshot={snapshot}
      statusPhase={statusPhase}
      liveText={liveText}
      onNavigate={onNavigate}
      onDownloadSuccess={onDownloadSuccess}
      refreshKey={refreshKey}
    />
  );
}
