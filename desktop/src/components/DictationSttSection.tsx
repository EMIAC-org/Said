import { useCallback, useEffect, useRef, useState } from "react";
import { Cloud, Download, HardDrive, Loader2, Trash2 } from "lucide-react";
import type { Preferences } from "../types";
import {
  chooseInstalledLocalModel,
  deleteAllLocalSpeechModels,
  getDesktopPrefs,
  getLocalModelInventory,
  getPreferences,
  getSttSetupPolicy,
  invoke,
  patchPreferences,
  removeUnusedLocalDictationModels,
  setDesktopPrefs,
  type DesktopPrefs,
  type DictationRoute,
  type LocalModelInfo,
  type LocalModelInventory,
  type SttSetupPolicy,
} from "../lib/invoke";
import {
  cancelLocalModelDownload,
  isCancelledDownload,
  startLocalModelDownload,
  useLocalModelDownloads,
  type LocalModelDownload,
} from "../lib/localModels";
import { ErrorNotice } from "./ErrorNotice";
import { LocalModelRow } from "./LocalModelRow";
import { friendlyError } from "../lib/friendlyError";
import { dictationRouteOptions } from "../lib/dictationCatalogue";

const POLISH_MODELS = [
  { key: "deepinfra-gemma-4-26b-a4b", label: "Gemma 4 26B A4B" },
  { key: "deepseek-v4-flash", label: "DeepSeek V4 Flash" },
] as const;
const S1_MODEL = "s1-mini-q4";

interface DictationSttSectionProps {
  onPrefsUpdated: (prefs: Preferences) => void;
}

interface Snapshot {
  policy: SttSetupPolicy;
  desktopPrefs: DesktopPrefs;
  prefs: Preferences;
  inventory: LocalModelInventory;
  s1Installed: boolean;
}

type SavingStage = "speech" | "cleanup" | "storage" | null;

function formatSize(bytes: number): string {
  if (bytes >= 1_000_000_000) return `${(bytes / 1_000_000_000).toFixed(1)} GB`;
  return `${Math.max(1, Math.round(bytes / 1_000_000))} MB`;
}

function Radio({ checked, disabled, label, name, onChange }: {
  checked: boolean;
  disabled?: boolean;
  label: string;
  name: string;
  onChange: () => void;
}) {
  return (
    <input
      type="radio"
      aria-label={label}
      name={name}
      checked={checked}
      disabled={disabled}
      onChange={onChange}
      className="mt-0.5 h-4 w-4 shrink-0 accent-[hsl(var(--primary))]"
    />
  );
}

function Status({ active }: { active: boolean }) {
  return active ? <span className="text-[11px] text-primary">Saving…</span> : null;
}

function downloadStatus(download: LocalModelDownload): string {
  if (download.status === "verifying") return "Verifying…";
  if (download.status === "retrying") return "Connection interrupted — retrying…";
  return download.percent === null ? "Starting…" : `Downloading · ${download.percent}%`;
}

/** Models settings: one authoritative snapshot for speech, cleanup, and storage. */
export function DictationSttSection({ onPrefsUpdated }: DictationSttSectionProps) {
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [savingStage, setSavingStage] = useState<SavingStage>(null);
  const [pendingDownload, setPendingDownload] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [confirmDeleteAll, setConfirmDeleteAll] = useState(false);
  const mounted = useRef(true);
  const generation = useRef(0);
  const mutation = useRef<SavingStage>(null);

  const refresh = useCallback(async () => {
    const requested = ++generation.current;
    try {
      if (mutation.current) return;
      const [policy, desktopPrefs, inventory, remotePrefs, s1Status] = await Promise.all([
        getSttSetupPolicy(),
        getDesktopPrefs(),
        getLocalModelInventory(),
        getPreferences(),
        invoke<{ installed: boolean }>("get_s1_mini_status").catch(() => ({ installed: false })),
      ]);
      if (!mounted.current || requested !== generation.current) return;
      if (!remotePrefs) throw new Error("Could not load cleanup settings. Please retry.");
      setSnapshot({
        policy,
        desktopPrefs,
        inventory,
        prefs: remotePrefs,
        s1Installed: s1Status.installed,
      });
    } catch (cause) {
      if (mounted.current && requested === generation.current) setError(friendlyError(cause));
    }
  }, []);

  const downloads = useLocalModelDownloads({
    onDone: () => void refresh(),
    onError: (_model, message) => setError(friendlyError(message)),
  });

  useEffect(() => {
    mounted.current = true;
    void refresh();
    return () => { mounted.current = false; };
  }, [refresh]);

  async function runMutation(stage: Exclude<SavingStage, null>, work: () => Promise<void>) {
    if (mutation.current) return;
    mutation.current = stage;
    setSavingStage(stage);
    setError("");
    setNotice("");
    generation.current += 1;
    try {
      await work();
    } catch (cause) {
      const message = friendlyError(cause);
      if (!isCancelledDownload(message)) setError(message);
    } finally {
      if (mutation.current === stage) {
        mutation.current = null;
        await refresh();
        if (mounted.current) setSavingStage(null);
      }
    }
  }

  const selectCloudSpeech = (route: DictationRoute) => {
    void runMutation("speech", async () => {
      const latest = await getDesktopPrefs();
      const next = { ...latest, dictation_stt: route };
      await setDesktopPrefs(next);
      setSnapshot((current) => current ? { ...current, desktopPrefs: next } : current);
      setNotice("Speech-to-text selection saved.");
    });
  };

  const selectLocalSpeech = (model: LocalModelInfo) => {
    if (!model.installed) return;
    void runMutation("speech", async () => {
      const nextInventory = await chooseInstalledLocalModel(model.key);
      setSnapshot((current) => current ? {
        ...current,
        inventory: nextInventory,
        desktopPrefs: { ...current.desktopPrefs, dictation_stt: "local", local_stt_model: nextInventory.selected_model },
      } : current);
      setNotice("Speech-to-text selection saved.");
    });
  };

  const downloadSpeech = (model: LocalModelInfo) => {
    if (mutation.current) return;
    setPendingDownload(model.key);
    void runMutation("speech", async () => {
      await startLocalModelDownload(model.key);
      setNotice(`${model.name} is downloaded. Select it when you are ready.`);
    }).finally(() => setPendingDownload((current) => current === model.key ? null : current));
  };

  const cancelSpeech = (model: LocalModelInfo) => {
    void cancelLocalModelDownload(model.key);
  };

  const setCleanupEnabled = (enabled: boolean) => {
    void runMutation("cleanup", async () => {
      const latest = await getDesktopPrefs();
      const next = { ...latest, polish_enabled: enabled };
      await setDesktopPrefs(next);
      setSnapshot((current) => current ? { ...current, desktopPrefs: next } : current);
      setNotice(enabled ? "Text cleanup is enabled." : "Text cleanup is paused.");
    });
  };

  const selectCleanupModel = (model: string) => {
    if (!snapshot?.desktopPrefs.polish_enabled) return;
    void runMutation("cleanup", async () => {
      const latestDesktopPrefs = await getDesktopPrefs();
      const updated = await patchPreferences({ selected_model: model }, { throwOnError: true });
      if (!updated) throw new Error("Could not save the cleanup model.");
      const nextPrefs = updated;
      setSnapshot((current) => current ? { ...current, desktopPrefs: latestDesktopPrefs, prefs: nextPrefs } : current);
      onPrefsUpdated(updated);
      setNotice("Cleanup model selection saved.");
    });
  };

  const downloadS1 = () => {
    if (mutation.current) return;
    setPendingDownload(S1_MODEL);
    void runMutation("cleanup", async () => {
      await startLocalModelDownload(S1_MODEL);
      setNotice("S1-mini is downloaded. Select it when you are ready.");
    }).finally(() => setPendingDownload((current) => current === S1_MODEL ? null : current));
  };

  const cancelS1 = () => { void cancelLocalModelDownload(S1_MODEL); };

  const removeUnused = () => {
    void runMutation("storage", async () => {
      const result = await removeUnusedLocalDictationModels();
      setNotice(result.removed.length > 0
        ? `Removed ${result.removed.map((model) => model.name).join(", ")} and freed ${formatSize(result.freed_bytes)}.`
        : "No unused local dictation models were found.");
    });
  };

  const deleteAll = () => {
    void runMutation("storage", async () => {
      const result = await deleteAllLocalSpeechModels();
      setNotice(result.removed.length > 0
        ? `Deleted all local speech models and freed ${formatSize(result.freed_bytes)}.`
        : "No local speech models were installed.");
      setConfirmDeleteAll(false);
    });
  };

  if (!snapshot) {
    return (
      <div className="panel mb-7 px-5 py-4">
        {error ? <><ErrorNotice error={error} /><button type="button" className="btn-ghost mt-2" onClick={() => void refresh()}>Retry loading settings</button></> : <p className="text-[12px] text-muted-foreground">Loading model settings…</p>}
      </div>
    );
  }
  const { policy, desktopPrefs, inventory, prefs: currentPrefs, s1Installed } = snapshot;
  const locked = savingStage !== null;
  const selectableModels = inventory.models
    .filter((model) => model.selectable)
    .sort((left, right) => Number(right.recommended) - Number(left.recommended));
  const localRouteSelected = desktopPrefs.dictation_stt === "local";
  const cleanupModel = currentPrefs?.selected_model ?? "";
  const cleanupLabel = cleanupModel === S1_MODEL
    ? "S1-mini"
    : POLISH_MODELS.find((model) => model.key === cleanupModel)?.label ?? "Configured model";
  const s1Download = downloads[S1_MODEL];
  const s1Pending = pendingDownload === S1_MODEL;
  const installed = inventory.models.filter((model) => model.installed);

  return (
    <div className="panel overflow-hidden mb-7">
      <div className="px-5 py-4 border-b" style={{ borderColor: "hsl(var(--surface-3))" }}>
        <p className="text-[12px] text-foreground">Speech: {localRouteSelected ? inventory.models.find((model) => model.key === desktopPrefs.local_stt_model)?.name ?? "Local model" : dictationRouteOptions(policy).find((option) => option.id === desktopPrefs.dictation_stt)?.label}</p>
        <p className="text-[12px] text-muted-foreground mt-1">Cleanup: {desktopPrefs.polish_enabled ? cleanupLabel : "Off · raw transcript"}</p>
      </div>

      <section className="px-5 py-4 border-b" style={{ borderColor: "hsl(var(--surface-3))" }}>
        <div className="flex items-center justify-between gap-3">
          <div>
            <p className="text-[13px] font-medium text-foreground">Speech-to-text</p>
            <p className="text-[11px] text-muted-foreground mt-1">Select one speech recognizer. Local models keep audio on this device.</p>
          </div>
          <Status active={savingStage === "speech"} />
        </div>
        <div role="radiogroup" aria-label="Speech-to-text model" className="mt-3 rounded-lg border overflow-hidden" style={{ borderColor: "hsl(var(--surface-3))" }}>
          {selectableModels.length > 0 && <div className="px-3 py-2 border-b text-[10px] font-medium uppercase tracking-wide text-muted-foreground" style={{ borderColor: "hsl(var(--surface-3))" }}>Local speech models</div>}
          {selectableModels.map((model, index) => {
            const download = downloads[model.key];
            const selected = localRouteSelected && model.key === desktopPrefs.local_stt_model;
            return (
              <div key={model.key} style={{ borderTop: index > 0 ? "1px solid hsl(var(--surface-3))" : undefined }}>
                <LocalModelRow model={model} selected={selected} disabled={locked} pending={pendingDownload === model.key} download={download}
                  onSelect={selectLocalSpeech} onDownload={downloadSpeech} onCancel={cancelSpeech} />
              </div>
            );
          })}
          <div className="px-3 py-2 border-y text-[10px] font-medium uppercase tracking-wide text-muted-foreground" style={{ borderColor: "hsl(var(--surface-3))" }}>Cloud speech models</div>
          {dictationRouteOptions(policy).filter((option) => option.kind === "cloud").map((option) => (
            <label key={option.id} className="px-3 py-3 flex items-start gap-2.5 cursor-pointer" style={{ borderTop: "1px solid hsl(var(--surface-3))" }}>
              <Radio checked={desktopPrefs.dictation_stt === option.id} disabled={locked} label={option.label} name="speech-model" onChange={() => selectCloudSpeech(option.id)} />
              <span className="min-w-0 flex-1"><span className="block text-[13px] font-medium text-foreground flex items-center gap-1.5"><Cloud size={14} /> {option.label}</span><span className="block text-[11px] text-muted-foreground mt-1">{option.provider} · {option.detail}</span></span>
            </label>
          ))}
        </div>
      </section>

      <section className="px-5 py-4 border-b" style={{ borderColor: "hsl(var(--surface-3))" }}>
        <div className="flex items-center justify-between gap-3">
          <div>
            <p className="text-[13px] font-medium text-foreground">Text cleanup</p>
            <p className="text-[11px] text-muted-foreground mt-1">{desktopPrefs.polish_enabled ? `On · ${cleanupLabel}` : `Selected · ${cleanupLabel} · paused`}</p>
          </div>
          <div className="flex items-center gap-2">
            <Status active={savingStage === "cleanup"} />
            <button type="button" role="switch" aria-checked={desktopPrefs.polish_enabled} aria-label="Enable text cleanup" disabled={locked} onClick={() => setCleanupEnabled(!desktopPrefs.polish_enabled)} className="relative h-6 w-11 shrink-0 rounded-full transition-colors" style={{ background: desktopPrefs.polish_enabled ? "hsl(var(--primary))" : "hsl(var(--surface-4))" }}>
              <span className="absolute top-0.5 h-5 w-5 rounded-full bg-white shadow transition-transform" style={{ left: 2, transform: desktopPrefs.polish_enabled ? "translateX(20px)" : "translateX(0)" }} />
            </button>
          </div>
        </div>
      </section>

      <section className="px-5 py-4 border-b" style={{ borderColor: "hsl(var(--surface-3))" }}>
        <div className="flex items-center justify-between gap-3">
          <div>
            <p className="text-[13px] font-medium text-foreground">Cleanup model</p>
            <p className="text-[11px] text-muted-foreground mt-1">Choose the model used after transcription. Your selection stays saved when cleanup is paused.</p>
          </div>
          <Status active={savingStage === "cleanup"} />
        </div>
        <div role="radiogroup" aria-label="Cleanup model" className={`mt-3 rounded-lg border overflow-hidden ${!desktopPrefs.polish_enabled ? "opacity-60" : ""}`} style={{ borderColor: "hsl(var(--surface-3))" }}>
          <div className="px-3 py-2.5 border-b" style={{ borderColor: "hsl(var(--surface-3))" }}><p className="text-[11px] font-medium text-muted-foreground uppercase tracking-wide">Local</p></div>
          <div className="px-3 py-3 flex items-start gap-2.5">
            <label className="flex min-w-0 flex-1 items-start gap-2.5 cursor-pointer">
            <Radio checked={cleanupModel === S1_MODEL} disabled={!desktopPrefs.polish_enabled || !s1Installed || locked} label="S1-mini by Superwhisper" name="cleanup-model" onChange={() => selectCleanupModel(S1_MODEL)} />
            <div className="min-w-0 flex-1"><p className="text-[13px] font-medium text-foreground">S1-mini by Superwhisper</p><p className="text-[11px] text-muted-foreground mt-1">On-device English text cleanup · ~484 MB</p><p className="text-[10px] text-muted-foreground mt-1">English only. Custom prompts, translation and Repair require a cloud model.</p></div>
            </label>
            {s1Download || s1Pending ? <button type="button" className="btn-ghost shrink-0" onClick={cancelS1}>Cancel</button> : s1Installed ? <span className="text-[11px] text-primary shrink-0">{cleanupModel === S1_MODEL ? desktopPrefs.polish_enabled ? "Selected" : "Selected · paused" : "Installed"}</span> : <button type="button" aria-label="Download S1-mini" className="btn-primary shrink-0" disabled={locked} onClick={downloadS1}><Download size={13} /> Download S1-mini</button>}
          </div>
          {(s1Download || s1Pending) && <div className="px-3 pb-3 pl-9"><p className="text-[11px] text-muted-foreground">{s1Download ? downloadStatus(s1Download) : "Starting download…"}</p></div>}
          <div className="px-3 py-2.5 border-y" style={{ borderColor: "hsl(var(--surface-3))" }}><p className="text-[11px] font-medium text-muted-foreground uppercase tracking-wide">Cloud</p></div>
          {POLISH_MODELS.map((model) => (
            <label key={model.key} className="px-3 py-3 flex items-start gap-2.5 cursor-pointer" style={{ borderTop: "1px solid hsl(var(--surface-3))" }}>
              <Radio checked={cleanupModel === model.key} disabled={!desktopPrefs.polish_enabled || locked} label={model.label} name="cleanup-model" onChange={() => selectCleanupModel(model.key)} />
              <span className="min-w-0 flex-1"><span className="block text-[13px] font-medium text-foreground">{model.label}</span><span className="block text-[11px] text-muted-foreground mt-1">Hosted text cleanup · internet required</span></span>
              {cleanupModel === model.key && <span className="text-[11px] text-primary shrink-0">{desktopPrefs.polish_enabled ? "Selected" : "Selected · paused"}</span>}
            </label>
          ))}
        </div>
      </section>

      <details className="px-5 py-3">
        <summary className="cursor-pointer list-none text-[12px] font-medium text-foreground flex items-center gap-1.5"><HardDrive size={14} /> Manage local model storage</summary>
        <div className="pt-3 pl-5">
          <p className="text-[11px] text-muted-foreground">{installed.length > 0 ? installed.map((model) => `${model.name} (${formatSize(model.size_bytes)})`).join(" · ") : "No local speech models are installed."}</p>
          {s1Installed && <p className="text-[11px] text-muted-foreground mt-1">S1-mini by Superwhisper · 484 MB · text cleanup</p>}
          {confirmDeleteAll && <p className="text-[11px] text-destructive mt-3" role="alert">Delete all speech models? Speech recognition will switch to cloud. Meetings will need Oriserve downloaded again. S1-mini will stay installed.</p>}
          <div className="mt-3 flex gap-2">
            {inventory.reclaimable_bytes > 0 && <button type="button" className="btn-ghost" disabled={locked} onClick={removeUnused}>{savingStage === "storage" ? <Loader2 size={13} className="animate-spin" /> : <Trash2 size={13} />} Free {formatSize(inventory.reclaimable_bytes)}</button>}
            {confirmDeleteAll ? <><button type="button" className="btn-ghost" disabled={locked} onClick={() => setConfirmDeleteAll(false)}>Cancel</button><button type="button" className="btn-ghost text-destructive" disabled={locked} onClick={deleteAll}>Delete all</button></> : <button type="button" className="text-[11px] text-destructive hover:underline" disabled={locked || installed.length === 0} onClick={() => setConfirmDeleteAll(true)}>Delete all local speech models</button>}
          </div>
        </div>
      </details>

      {(notice || error) && <div className="px-5 pb-4">{notice && <p className="text-[11px] text-primary" role="status">{notice}</p>}<ErrorNotice error={error} onRetry={() => void refresh()} /></div>}
    </div>
  );
}
