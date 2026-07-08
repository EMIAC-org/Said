import { Check, Loader2, Trash2 } from "lucide-react";

// Result of the `reclaim_old_models` Tauri command.
export interface ReclaimResult {
  removed: { name: string; size_bytes: number }[];
  freed_bytes: number;
}

function formatSize(bytes: number): string {
  if (bytes >= 1e9) return `${(bytes / 1e9).toFixed(1)} GB`;
  if (bytes >= 1e6) return `${Math.round(bytes / 1e6)} MB`;
  if (bytes > 0) return `${Math.round(bytes / 1e3)} KB`;
  return "—";
}

/** Free up space by removing unsupported extra speech models. */
export function ReclaimOldModelsRow({
  reclaiming,
  result,
  error,
  onReclaim,
}: {
  reclaiming: boolean;
  result: ReclaimResult | null;
  error: string;
  onReclaim: () => void;
}) {
  if (result) {
    const freed = result.freed_bytes > 0 ? formatSize(result.freed_bytes) : null;
    return (
      <div className="onb-reclaim onb-reclaim-done">
        <Check size={13} />
        <span>
          {result.removed.length > 0
            ? `Extra speech models removed${freed ? ` — freed ${freed}` : ""}.`
            : "Already clean — no extra speech models to remove."}
        </span>
      </div>
    );
  }
  return (
    <div className="onb-reclaim">
      <div className="onb-reclaim-main">
        <div className="onb-reclaim-copy">
          <Trash2 size={13} />
          <span>Free up space by removing extra speech models you no longer need.</span>
        </div>
        <button
          type="button"
          onClick={onReclaim}
          disabled={reclaiming}
          className="btn-ghost text-[11px] shrink-0"
          style={{ height: 26 }}
        >
          {reclaiming ? <Loader2 size={12} className="animate-spin" /> : null}
          {reclaiming ? "Cleaning…" : "Free up space"}
        </button>
      </div>
      {error && <p className="onb-reclaim-error">{error}</p>}
    </div>
  );
}
