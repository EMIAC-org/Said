import { useEffect } from "react";
import { X, RotateCcw, History, Download } from "lucide-react";

// ── Retry Toast ───────────────────────────────────────────────────────────────
//
// Surfaces a recording / STT / polish failure with three affordances:
//   • Retry        — re-runs the pipeline with the saved WAV (only if audioId)
//   • Open history — navigates to the History view to inspect what landed
//   • Dismiss      — closes the toast
//
// Retry is disabled (greyed out) when no audioId is available so users
// understand why it's missing instead of silently no-op'ing.

interface RetryToastProps {
  message:        string;
  canRetry:       boolean;
  onRetry:        () => void;
  onOpenHistory:  () => void;
  onDismiss:      () => void;
}

export function RetryToast({
  message, canRetry, onRetry, onOpenHistory, onDismiss,
}: RetryToastProps) {
  return (
    <div
      className="fixed bottom-5 left-1/2 -translate-x-1/2 z-50 flex items-center gap-3 px-4 py-3 rounded-2xl shadow-xl max-w-md w-max"
      style={{
        background:  "hsl(var(--surface-3))",
        border:      "1px solid hsl(var(--border))",
        boxShadow:   "0 8px 32px hsl(0 0% 0% / 0.28)",
        animation:   "fadeIn 0.18s ease-out",
      }}
    >
      {/* Red accent circle */}
      <span
        className="w-7 h-7 rounded-full flex items-center justify-center flex-shrink-0"
        style={{ background: "hsl(var(--chip-red-bg))", color: "hsl(var(--chip-red-fg))" }}
      >
        <X size={13} strokeWidth={2.5} />
      </span>

      {/* Two-line message */}
      <div className="flex-1 min-w-0">
        <p className="text-[12px] font-semibold text-foreground leading-tight">
          Recording failed
        </p>
        <p className="text-[11px] text-muted-foreground leading-tight mt-0.5 truncate" title={message}>
          {message}
        </p>
      </div>

      {/* Actions */}
      <div className="flex items-center gap-1.5 flex-shrink-0">
        <button
          onClick={onOpenHistory}
          title="Open history"
          className="flex items-center gap-1 px-2.5 py-1 rounded-lg text-[11px] font-semibold transition-colors"
          style={{
            background: "hsl(var(--surface-4))",
            color:      "hsl(var(--foreground))",
          }}
          onMouseEnter={(e) => { e.currentTarget.style.background = "hsl(var(--surface-hover))"; }}
          onMouseLeave={(e) => { e.currentTarget.style.background = "hsl(var(--surface-4))"; }}
        >
          <History size={11} />
          History
        </button>
        <button
          onClick={onRetry}
          disabled={!canRetry}
          title={canRetry ? "Retry the recording" : "No saved audio to retry"}
          className="flex items-center gap-1 px-2.5 py-1 rounded-lg text-[11px] font-semibold transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
          style={{
            background: "hsl(var(--primary))",
            color:      "hsl(var(--primary-foreground))",
          }}
        >
          <RotateCcw size={11} />
          Retry
        </button>
        <button
          onClick={onDismiss}
          title="Dismiss"
          className="w-6 h-6 rounded-lg flex items-center justify-center transition-colors opacity-50 hover:opacity-100"
          style={{ background: "hsl(var(--surface-4))" }}
        >
          <X size={11} />
        </button>
      </div>
    </div>
  );
}

// ── Download Success Toast ───────────────────────────────────────────────────

interface DownloadSuccessToastProps {
  path:      string;
  onReveal:  () => void;
  onDismiss: () => void;
}

export function DownloadSuccessToast({ path, onReveal, onDismiss }: DownloadSuccessToastProps) {
  useEffect(() => {
    const t = setTimeout(onDismiss, 7000);
    return () => clearTimeout(t);
  }, [onDismiss]);

  return (
    <div
      className="fixed bottom-5 left-1/2 -translate-x-1/2 z-50 flex items-center gap-3 px-4 py-3 rounded-2xl shadow-xl max-w-sm"
      style={{
        background:  "hsl(var(--surface-3))",
        border:      "1px solid hsl(var(--border))",
        boxShadow:   "0 8px 32px hsl(0 0% 0% / 0.28)",
        animation:   "fadeIn 0.18s ease-out",
      }}
    >
      <span
        className="w-7 h-7 rounded-full flex items-center justify-center flex-shrink-0"
        style={{ background: "hsl(var(--chip-mint-bg))", color: "hsl(var(--chip-mint-fg))" }}
      >
        <Download size={12} strokeWidth={2.5} />
      </span>

      <div className="flex-1 min-w-0">
        <p className="text-[12px] font-semibold text-foreground leading-tight">
          Download complete
        </p>
        <p className="text-[11px] text-muted-foreground leading-tight mt-0.5 truncate" title={path}>
          Saved to {path}
        </p>
      </div>

      <button
        onClick={onReveal}
        title="Show in Finder"
        className="flex items-center gap-1 px-2.5 py-1 rounded-lg text-[11px] font-semibold transition-colors flex-shrink-0"
        style={{
          background: "hsl(var(--surface-4))",
          color:      "hsl(var(--foreground))",
        }}
        onMouseEnter={(e) => { e.currentTarget.style.background = "hsl(var(--surface-hover))"; }}
        onMouseLeave={(e) => { e.currentTarget.style.background = "hsl(var(--surface-4))"; }}
      >
        Show
      </button>

      <button
        onClick={onDismiss}
        title="Dismiss"
        className="w-6 h-6 rounded-lg flex items-center justify-center transition-colors opacity-50 hover:opacity-100 flex-shrink-0"
        style={{ background: "hsl(var(--surface-4))" }}
      >
        <X size={11} />
      </button>
    </div>
  );
}
