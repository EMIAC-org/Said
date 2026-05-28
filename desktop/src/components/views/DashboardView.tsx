import type { AppSnapshot, PendingEdit } from "@/types";
import { EditorialDashboard } from "@/components/views/dashboards/EditorialDashboard";

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
}: DashboardViewProps) {
  return (
    <EditorialDashboard
      snapshot={snapshot}
      statusPhase={statusPhase}
      liveText={liveText}
    />
  );
}
