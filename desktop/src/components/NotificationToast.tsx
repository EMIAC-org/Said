import { useEffect } from "react";
import { X, RotateCcw, Check, Sparkles, BookOpen, Star, Undo2, History, Download } from "lucide-react";
import { cn } from "@/lib/utils";

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
        style={{ background: "hsl(0 70% 60% / 0.16)", color: "hsl(0 70% 60%)" }}
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
  filename:  string;
  onDismiss: () => void;
}

export function DownloadSuccessToast({ filename, onDismiss }: DownloadSuccessToastProps) {
  useEffect(() => {
    const t = setTimeout(onDismiss, 4000);
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
        <p className="text-[11px] text-muted-foreground leading-tight mt-0.5 truncate" title={filename}>
          Saved {filename} to Downloads.
        </p>
      </div>

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

// ── Edit Confirmation Toast ───────────────────────────────────────────────────

interface EditToastProps {
  aiOutput:    string;
  userKept:    string;
  onSave:      () => void;
  onDismiss:   () => void;
}

export function EditConfirmToast({ aiOutput, userKept, onSave, onDismiss }: EditToastProps) {
  const aiShort   = aiOutput.length  > 60 ? aiOutput.slice(0, 60)  + "…" : aiOutput;
  const keptShort = userKept.length  > 60 ? userKept.slice(0, 60)  + "…" : userKept;

  return (
    <div
      className="fixed bottom-5 left-1/2 -translate-x-1/2 z-50 flex flex-col gap-3 px-4 py-3.5 rounded-2xl shadow-xl"
      style={{
        background:  "hsl(var(--surface-3))",
        border:      "1px solid hsl(var(--border))",
        boxShadow:   "0 8px 32px hsl(0 0% 0% / 0.28)",
        minWidth:    "280px",
        maxWidth:    "360px",
      }}
    >
      {/* Header */}
      <div className="flex items-center justify-between gap-2">
        <p className="text-[12px] font-semibold text-foreground">Save this edit?</p>
        <button
          onClick={onDismiss}
          className="w-5 h-5 rounded flex items-center justify-center opacity-50 hover:opacity-100 transition-opacity"
        >
          <X size={10} />
        </button>
      </div>

      {/* Diff preview */}
      <div className="flex flex-col gap-1.5 text-[11px]">
        <div className="flex gap-2 items-start">
          <span className="flex-shrink-0 font-semibold opacity-50 w-5 text-right">AI</span>
          <span
            className="flex-1 leading-snug px-2 py-1 rounded-lg text-muted-foreground line-through"
            style={{ background: "hsl(0 50% 50% / 0.1)" }}
            title={aiOutput}
          >
            {aiShort}
          </span>
        </div>
        <div className="flex gap-2 items-start">
          <span className="flex-shrink-0 font-semibold opacity-50 w-5 text-right">You</span>
          <span
            className="flex-1 leading-snug px-2 py-1 rounded-lg text-foreground"
            style={{ background: "hsl(var(--chip-lime-fg) / 0.1)", color: "hsl(var(--chip-lime-fg))" }}
            title={userKept}
          >
            {keptShort}
          </span>
        </div>
      </div>

      <p className="text-[10px] text-muted-foreground leading-relaxed opacity-70">
        Said will remember this style for future recordings.
      </p>

      {/* Buttons */}
      <div className={cn("flex gap-2")}>
        <button
          onClick={onDismiss}
          className="flex-1 py-1.5 rounded-xl text-[11px] font-medium transition-colors text-muted-foreground"
          style={{ background: "hsl(var(--surface-4))" }}
        >
          Skip
        </button>
        <button
          onClick={onSave}
          className="flex-1 py-1.5 rounded-xl text-[11px] font-semibold transition-colors flex items-center justify-center gap-1"
          style={{
            background: "hsl(var(--primary))",
            color:      "hsl(var(--primary-foreground))",
          }}
        >
          <Check size={11} />
          Save preference
        </button>
      </div>
    </div>
  );
}

// ── Vocabulary Added Toast ────────────────────────────────────────────────────
//
// Replaces the previous OS-level "looks like a log" notification with an
// in-app toast that matches RetryToast's design exactly: bottom-center,
// surface-3 panel, drop shadow, dismiss + primary action affordances.
//
// Variants:
//   • "added"   — auto-promoted from STT_ERROR or manually added
//   • "starred" — pinned by the user
//   • "removed" — deleted (used by Undo confirmation)
//
// Auto-dismisses after 6 seconds; the host (App.tsx) controls visibility.

export type VocabToastKind = "added" | "starred" | "removed" | "queued";

interface VocabToastProps {
  kind:     VocabToastKind;
  term:     string;
  source?:  "auto" | "manual" | "starred";   // present for "added"
  onUndo?:  () => void;
  onDismiss: () => void;
}

export function VocabularyToast({ kind, term, source, onUndo, onDismiss }: VocabToastProps) {
  // Auto-dismiss after 6s. User can interact with Undo before then.
  useEffect(() => {
    const t = setTimeout(onDismiss, 6000);
    return () => clearTimeout(t);
  }, [onDismiss]);

  // Icon + accent color per kind ───────────────────────────────────────────
  const accent = (() => {
    if (kind === "starred") {
      return {
        icon:  <Star size={11} fill="currentColor" />,
        color: "hsl(var(--chip-amber-fg))",
        bg:    "hsl(var(--chip-amber-bg))",
      };
    }
    if (kind === "removed") {
      return {
        icon:  <BookOpen size={11} />,
        color: "hsl(var(--muted-foreground))",
        bg:    "hsl(var(--surface-4))",
      };
    }
    if (kind === "queued") {
      // Quieter palette than "added" — we noticed but haven't committed yet.
      return {
        icon:  <Sparkles size={11} />,
        color: "hsl(var(--muted-foreground))",
        bg:    "hsl(var(--surface-4))",
      };
    }
    // added — sparkle if auto, plus if manual
    return {
      icon:  source === "manual"
        ? <BookOpen size={11} />
        : <Sparkles size={11} />,
      color: "hsl(var(--chip-mint-fg))",
      bg:    "hsl(var(--chip-mint-bg))",
    };
  })();

  // Headline text per kind ─────────────────────────────────────────────────
  const headline = (() => {
    if (kind === "starred") return "Pinned to vocabulary";
    if (kind === "removed") return "Removed from vocabulary";
    if (kind === "queued")  return "Noticed your correction";
    return source === "manual"
      ? "Added to vocabulary"
      : "Said learned a new word";
  })();

  const subtle = (() => {
    if (kind === "starred") return "Said will keep this even if you stop using it.";
    if (kind === "removed") return "Said won't recognise this any more.";
    if (kind === "queued")  return "Make this fix once more and Said will remember it.";
    return source === "manual"
      ? "Said will recognise this on your next recording."
      : "Said remembered your correction.";
  })();

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
      {/* Accent circle with kind icon */}
      <span
        className="w-7 h-7 rounded-full flex items-center justify-center flex-shrink-0"
        style={{ background: accent.bg, color: accent.color }}
      >
        {accent.icon}
      </span>

      {/* Two-line message */}
      <div className="flex-1 min-w-0">
        <p className="text-[12px] font-semibold text-foreground leading-tight">
          {headline}
        </p>
        <p className="text-[11px] text-muted-foreground leading-tight mt-0.5 truncate">
          <span
            className="font-mono px-1.5 py-0.5 rounded"
            style={{ background: "hsl(var(--surface-4))" }}
          >
            {term}
          </span>
          <span className="ml-1.5">· {subtle}</span>
        </p>
      </div>

      {/* Actions */}
      <div className="flex items-center gap-1.5 flex-shrink-0">
        {onUndo && (
          <button
            onClick={onUndo}
            className="flex items-center gap-1 px-2.5 py-1 rounded-lg text-[11px] font-semibold transition-colors"
            style={{
              background: "hsl(var(--surface-4))",
              color:      "hsl(var(--foreground))",
            }}
            onMouseEnter={(e) => { e.currentTarget.style.background = "hsl(var(--surface-hover))"; }}
            onMouseLeave={(e) => { e.currentTarget.style.background = "hsl(var(--surface-4))"; }}
          >
            <Undo2 size={11} />
            Undo
          </button>
        )}
        <button
          onClick={onDismiss}
          className="w-6 h-6 rounded-lg flex items-center justify-center transition-colors opacity-50 hover:opacity-100"
          style={{ background: "hsl(var(--surface-4))" }}
        >
          <X size={11} />
        </button>
      </div>
    </div>
  );
}
