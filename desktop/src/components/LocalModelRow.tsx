import { Download } from "lucide-react";
import type { LocalModelInfo } from "../lib/invoke";
import type { LocalModelDownload } from "../lib/localModels";

interface LocalModelRowProps {
  model: LocalModelInfo;
  selected: boolean;
  disabled?: boolean;
  pending?: boolean;
  download?: LocalModelDownload;
  onSelect: (model: LocalModelInfo) => void;
  onDownload: (model: LocalModelInfo) => void;
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

export function LocalModelRow({
  model,
  selected,
  disabled = false,
  pending = false,
  download,
  onSelect,
  onDownload,
  onCancel,
}: LocalModelRowProps) {
  const downloading = pending || download !== undefined;

  return (
    <div className="px-3 py-3 flex items-start gap-2.5" aria-live={downloading ? "polite" : undefined}>
      <label className="flex min-w-0 flex-1 items-start gap-2.5 cursor-pointer">
      <input
        type="radio"
        aria-label={model.name}
        name="speech-model"
        checked={selected}
        disabled={disabled || !model.installed}
        onChange={() => onSelect(model)}
        className="mt-0.5 h-4 w-4 shrink-0 accent-[hsl(var(--primary))]"
      />
      <div className="min-w-0 flex-1">
        <p className="text-[13px] font-medium text-foreground flex flex-wrap items-center gap-1.5">
          {model.name}
          {model.recommended && <span className="rounded-full px-1.5 py-0.5 text-[10px] font-medium leading-none" style={{ background: "hsl(var(--primary) / 0.14)", color: "hsl(var(--primary))" }}>Recommended</span>}
        </p>
        <p className="text-[11px] text-muted-foreground mt-1">{languageSummary(model)} · {model.streaming ? "Live local transcript" : "Batch transcription"} · {model.size_hint}</p>
        {downloading && <p className="text-[11px] text-muted-foreground mt-1">{download ? statusLabel(download) : "Starting download…"}</p>}
      </div>
      </label>
      {downloading ? (
        <button type="button" className="btn-ghost shrink-0" onClick={() => onCancel(model)}>Cancel</button>
      ) : model.installed ? (
        <span className={`text-[11px] shrink-0 ${selected ? "text-primary" : "text-muted-foreground"}`}>{selected ? "Selected" : "Installed"}</span>
      ) : (
        <button type="button" className="btn-primary shrink-0" aria-label={`Download ${model.name}`} disabled={disabled} onClick={() => onDownload(model)}><Download size={13} /> Download</button>
      )}
    </div>
  );
}
