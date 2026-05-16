import { useState } from "react";
import { Upload, Loader2, Check, AlertCircle, RotateCcw } from "lucide-react";
import { syncMeetingToLark } from "@/lib/enterprise";

// ── Types ──────────────────────────────────────────────────────────────────────

interface LarkSyncButtonProps {
  meetingId: string;
  disabled?: boolean;
}

type SyncState =
  | { kind: "idle" }
  | { kind: "syncing" }
  | { kind: "done"; tasks_synced: number; doc_id?: string; messages_sent: number }
  | { kind: "error" };

// ── Component ──────────────────────────────────────────────────────────────────

export function LarkSyncButton({ meetingId, disabled }: LarkSyncButtonProps) {
  const [state, setState] = useState<SyncState>({ kind: "idle" });

  async function handleSync() {
    setState({ kind: "syncing" });
    const result = await syncMeetingToLark(meetingId);
    if (result) {
      setState({
        kind: "done",
        tasks_synced: result.tasks_synced,
        doc_id: result.doc_id,
        messages_sent: result.messages_sent,
      });
    } else {
      setState({ kind: "error" });
    }
  }

  // ── Syncing ──────────────────────────────────────────────────────────────────

  if (state.kind === "syncing") {
    return (
      <button
        disabled
        className="btn-primary"
        style={{ gap: "8px", cursor: "not-allowed" }}
      >
        <Loader2 size={14} className="animate-spin" />
        Syncing...
      </button>
    );
  }

  // ── Done ──────────────────────────────────────────────────────────────────────

  if (state.kind === "done") {
    const docLabel = state.doc_id ? "1 doc" : "0 docs";
    return (
      <div
        className="inline-flex items-center gap-2 px-3 py-2 rounded-lg text-[12.5px] font-medium"
        style={{
          background: "hsl(145 60% 12%)",
          color: "hsl(145 70% 65%)",
        }}
      >
        <Check size={14} />
        <span>
          Synced: {state.tasks_synced} task{state.tasks_synced !== 1 ? "s" : ""},{" "}
          {docLabel}, {state.messages_sent} notification{state.messages_sent !== 1 ? "s" : ""}
        </span>
      </div>
    );
  }

  // ── Error ─────────────────────────────────────────────────────────────────────

  if (state.kind === "error") {
    return (
      <div className="inline-flex items-center gap-3">
        <span
          className="inline-flex items-center gap-1.5 text-[12.5px] font-medium"
          style={{ color: "hsl(0 85% 68%)" }}
        >
          <AlertCircle size={14} />
          Sync failed. Try again.
        </span>
        <button
          onClick={handleSync}
          disabled={disabled}
          className="btn-ghost"
          style={{ gap: "6px", height: "32px", fontSize: "12px", padding: "0 12px" }}
        >
          <RotateCcw size={13} />
          Retry
        </button>
      </div>
    );
  }

  // ── Idle (default) ────────────────────────────────────────────────────────────

  return (
    <button
      onClick={handleSync}
      disabled={disabled}
      className="btn-primary"
      style={{ gap: "8px" }}
    >
      <Upload size={14} />
      Sync to Lark
    </button>
  );
}
