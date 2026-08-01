import { Check, Download, Loader2 } from "lucide-react";
import type { LocalModelInfo } from "../lib/invoke";
import type { LocalModelDownload } from "../lib/localModels";

interface LocalModelRowProps {
  model: LocalModelInfo;
  /** This model's own download, if one is running. Never another model's. */
  download?: LocalModelDownload;
  /** This row's own action is in flight. */
  pending: boolean;
  /** Another row owns the current action, so this row's controls are inert. */
  locked: boolean;
  onInstall: (model: LocalModelInfo) => void;
  onUse: (model: LocalModelInfo) => void;
  onCancel: (model: LocalModelInfo) => void;
}

function languageSummary(model: LocalModelInfo): string {
  if (model.languages.length === 1 && model.languages[0] === "en") return "English only";
  if (model.languages.includes("hi")) return "English, Hindi and multilingual";
  return model.languages.join(", ");
}

function statusLabel(download: LocalModelDownload): string {
  if (download.status === "verifying") return "Verifying…";
  if (download.status === "retrying") return "Connection interrupted — retrying…";
  return download.percent === null ? "Starting…" : `Downloading · ${download.percent}%`;
}

function Badge({ children, tone }: { children: string; tone: "primary" | "muted" }) {
  return (
    <span
      className="rounded-full px-1.5 py-0.5 text-[10px] font-medium leading-none"
      style={
        tone === "primary"
          ? { background: "hsl(var(--primary) / 0.14)", color: "hsl(var(--primary))" }
          : { background: "hsl(var(--surface-3))", color: "hsl(var(--muted-foreground))" }
      }
    >
      {children}
    </span>
  );
}

/**
 * One local speech model, owning everything about itself: its badges, its own
 * download progress, and its own action. A model never renders another model's
 * progress or spinner, so a download can only ever appear on the row that
 * started it.
 */
export function LocalModelRow({
  model,
  download,
  pending,
  locked,
  onInstall,
  onUse,
  onCancel,
}: LocalModelRowProps) {
  const downloading = download !== undefined;
  const busy = pending || downloading;

  return (
    <div className="px-4 py-3" aria-live={downloading ? "polite" : undefined}>
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <p className="text-[13px] font-medium text-foreground flex flex-wrap items-center gap-1.5">
            {model.name}
            {model.recommended && <Badge tone="primary">Recommended</Badge>}
            {model.required_for_meetings && <Badge tone="muted">Used by Meetings</Badge>}
            {model.installed && !model.active_for_dictation && <Badge tone="muted">Installed</Badge>}
          </p>
          <p className="text-[11px] text-muted-foreground mt-1">
            {languageSummary(model)}
            {model.streaming ? " · Live local transcript" : " · Batch transcription"}
            {` · ${model.size_hint}`}
          </p>
        </div>

        {downloading ? (
          <button type="button" className="btn-ghost shrink-0" onClick={() => onCancel(model)}>
            Cancel
          </button>
        ) : model.active_for_dictation ? (
          <span className="text-[11px] inline-flex shrink-0 items-center gap-1 text-primary">
            <Check size={13} /> Active
          </span>
        ) : model.installed ? (
          <button
            type="button"
            className="btn-ghost shrink-0"
            disabled={busy || locked}
            onClick={() => onUse(model)}
          >
            {pending ? <Loader2 size={14} className="animate-spin" /> : <Check size={14} />}
            Use
          </button>
        ) : (
          <button
            type="button"
            className="btn-primary shrink-0"
            disabled={busy || locked}
            onClick={() => onInstall(model)}
          >
            {pending ? <Loader2 size={14} className="animate-spin" /> : <Download size={14} />}
            Download {model.size_hint}
          </button>
        )}
      </div>

      {downloading && (
        <div className="mt-2.5">
          <div
            className="h-1 w-full overflow-hidden rounded-full"
            style={{ background: "hsl(var(--surface-3))" }}
          >
            <div
              className="h-full rounded-full transition-[width] duration-200"
              style={{
                width: `${Math.max(4, download.percent ?? 4)}%`,
                background: "hsl(var(--primary))",
              }}
            />
          </div>
          <p className="text-[11px] text-muted-foreground mt-1.5">
            {statusLabel(download)} · partial progress is kept if cancelled
          </p>
        </div>
      )}
    </div>
  );
}
