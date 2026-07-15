import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { Check, Cloud, Cpu, Download, HardDrive, Loader2, Trash2 } from "lucide-react";
import type { Preferences } from "../types";
import {
  chooseInstalledLocalModel,
  deleteAllLocalSpeechModels,
  getDesktopPrefs,
  getLocalModelInventory,
  getSttSetupPolicy,
  invoke,
  removeUnusedLocalDictationModels,
  setDesktopPrefs,
  type LocalModelInventory,
  type LocalModelKey,
  type SttSetupPolicy,
} from "../lib/invoke";
import { ErrorNotice } from "./ErrorNotice";
import { friendlyError } from "../lib/friendlyError";

interface DownloadProgress {
  name: string;
  received: number;
  total: number;
  status: "downloading" | "done" | "cancelled" | "error" | string;
  error: string | null;
}

interface DictationSttSectionProps {
  prefs: Preferences | null;
  onPrefsUpdated: (prefs: Preferences) => void;
  platform: string;
}

function localModelCommand(model: LocalModelKey) {
  if (model === "nemotron-q4") {
    return {
      download: "download_nemotron_model",
      args: { variant: "q4" },
      event: "nemotron-model-download",
      eventName: "nemotron-3.5-asr-streaming-0.6b-Q4_K_M.gguf",
    };
  }
  return {
    download: "download_dictation_model",
    args: undefined,
    event: "meeting-model-download",
    eventName: "ggml-oriserve-hinglish-fp16.bin",
  };
}

function formatSize(bytes: number): string {
  if (bytes >= 1_000_000_000) return `${(bytes / 1_000_000_000).toFixed(1)} GB`;
  return `${Math.max(1, Math.round(bytes / 1_000_000))} MB`;
}

/** Device-policy speech controls plus explicit, dependency-aware model storage. */
export function DictationSttSection({ prefs: _prefs, onPrefsUpdated: _onPrefsUpdated, platform: _platform }: DictationSttSectionProps) {
  const [policy, setPolicy] = useState<SttSetupPolicy | null>(null);
  const [desktopPrefs, setDesktopPrefsState] = useState<Awaited<ReturnType<typeof getDesktopPrefs>> | null>(null);
  const [inventory, setInventory] = useState<LocalModelInventory | null>(null);
  const [download, setDownload] = useState<DownloadProgress | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [confirmDeleteAll, setConfirmDeleteAll] = useState(false);
  const mounted = useRef(true);

  const refresh = useCallback(async () => {
    try {
      const [nextPolicy, nextPrefs, nextInventory] = await Promise.all([
        getSttSetupPolicy(),
        getDesktopPrefs(),
        getLocalModelInventory(),
      ]);
      if (!mounted.current) return;
      setPolicy(nextPolicy);
      setDesktopPrefsState(nextPrefs);
      setInventory(nextInventory);
    } catch (cause) {
      if (mounted.current) setError(friendlyError(cause));
    }
  }, []);

  useEffect(() => {
    mounted.current = true;
    void refresh();
    return () => { mounted.current = false; };
  }, [refresh]);

  useEffect(() => {
    const recommended = inventory?.recommended_model;
    if (!recommended || inventory?.setup_kind !== "local_required") return;
    const command = localModelCommand(recommended);
    const unlisten = listen<DownloadProgress>(command.event, (event) => {
      const progress = event.payload;
      if (progress.name !== command.eventName) return;
      if (progress.status === "downloading") {
        setDownload(progress);
        setError("");
      } else {
        setDownload(null);
      }
      if (progress.status === "done") void refresh();
      if (progress.status === "error" && progress.error) setError(friendlyError(progress.error));
    });
    return () => { void unlisten.then((stop) => stop()); };
  }, [inventory?.recommended_model, inventory?.setup_kind, refresh]);

  const selectRoute = useCallback(async (route: "local" | "cloud-nemotron-3.5") => {
    if (!desktopPrefs || desktopPrefs.dictation_stt === route) return;
    setError("");
    setNotice("");
    try {
      if (route === "local") {
        const selected = inventory?.models.find((model) => model.key === inventory.selected_model);
        const fallback = inventory?.models.find((model) => model.recommended && model.installed);
        const model = selected?.installed ? selected : fallback;
        if (!model) throw new Error("Download a local model before switching to local dictation.");
        await chooseInstalledLocalModel(model.key);
      } else {
        const next = { ...desktopPrefs, dictation_stt: route };
        await setDesktopPrefs(next);
      }
      await refresh();
    } catch (cause) {
      setError(friendlyError(cause));
      await refresh();
    }
  }, [desktopPrefs, inventory, refresh]);

  const downloadRecommended = useCallback(async () => {
    const recommended = inventory?.recommended_model;
    if (!recommended) return;
    const command = localModelCommand(recommended);
    setBusy(true);
    setError("");
    setNotice("");
    try {
      await invoke(command.download, command.args);
      await chooseInstalledLocalModel(recommended);
      setNotice(`${policy?.local_model_name ?? "Recommended model"} is installed and selected.`);
      await refresh();
    } catch (cause) {
      setError(friendlyError(cause));
    } finally {
      setBusy(false);
    }
  }, [inventory?.recommended_model, policy?.local_model_name, refresh]);

  const useRecommended = useCallback(async () => {
    const recommended = inventory?.recommended_model;
    if (!recommended) return;
    setBusy(true);
    setError("");
    try {
      await chooseInstalledLocalModel(recommended);
      setNotice(`${policy?.local_model_name ?? "Recommended model"} is selected.`);
      await refresh();
    } catch (cause) {
      setError(friendlyError(cause));
    } finally {
      setBusy(false);
    }
  }, [inventory?.recommended_model, policy?.local_model_name, refresh]);

  const removeUnused = useCallback(async () => {
    setBusy(true);
    setError("");
    setNotice("");
    try {
      const result = await removeUnusedLocalDictationModels();
      setNotice(result.removed.length > 0
        ? `Removed ${result.removed.map((model) => model.name).join(", ")} and freed ${formatSize(result.freed_bytes)}.`
        : "No unused local dictation models were found.");
      await refresh();
    } catch (cause) {
      setError(friendlyError(cause));
      await refresh();
    } finally {
      setBusy(false);
    }
  }, [refresh]);

  const deleteAll = useCallback(async () => {
    setBusy(true);
    setError("");
    setNotice("");
    try {
      const result = await deleteAllLocalSpeechModels();
      setNotice(result.removed.length > 0
        ? `Deleted all local speech models and freed ${formatSize(result.freed_bytes)}.`
        : "No local speech models were installed.");
      setConfirmDeleteAll(false);
      await refresh();
    } catch (cause) {
      setError(friendlyError(cause));
      await refresh();
    } finally {
      setBusy(false);
    }
  }, [refresh]);

  if (!policy || !inventory || !desktopPrefs) return null;

  const localSelected = desktopPrefs.dictation_stt !== "cloud-nemotron-3.5";
  const recommended = inventory.models.find((model) => model.key === inventory.recommended_model);
  const active = inventory.models.find((model) => model.active_for_dictation);
  const installed = inventory.models.filter((model) => model.installed);
  const progress = download && download.total > 0
    ? Math.min(100, Math.round((download.received / download.total) * 100))
    : null;

  return (
    <div className="panel overflow-hidden mb-7">
      <div className="px-5 py-4 border-b" style={{ borderColor: "hsl(var(--surface-3))" }}>
        <p className="text-[13px] font-medium text-foreground">Speech recognition</p>
        <p className="text-[12px] text-muted-foreground mt-0.5">
          {policy.setup_kind === "cloud_locked"
            ? "Live Nemotron is fixed for dictation on this device. Local files are used only by Meetings."
            : `This Mac recommends ${policy.local_model_name ?? "local speech recognition"}.`}
        </p>
      </div>

      <div className="px-5 py-4 flex flex-col gap-3">
        {policy.setup_kind === "local_required" ? (
          <div className="rounded-xl border overflow-hidden" style={{ borderColor: "hsl(var(--surface-3))", background: "hsl(var(--surface-2))" }}>
            {[
              {
                id: "local" as const,
                icon: <Cpu size={14} />,
                label: "Local",
                description: `${active?.name ?? recommended?.name ?? "Recommended local model"} · private and no per-use speech cost`,
              },
              {
                id: "cloud-nemotron-3.5" as const,
                icon: <Cloud size={14} />,
                label: "Cloud Nemotron",
                description: "Live multilingual speech recognition. Internet required.",
              },
            ].map((option, index) => {
              const selected = option.id === "local" ? localSelected : !localSelected;
              return (
                <button
                  key={option.id}
                  type="button"
                  onClick={() => void selectRoute(option.id)}
                  className="w-full text-left px-3 py-3 flex items-start gap-2.5 transition-colors hover:bg-black/5 dark:hover:bg-white/5"
                  style={index > 0 ? { borderTop: "1px solid hsl(var(--surface-3))" } : undefined}
                >
                  <span className="mt-0.5 flex h-4 w-4 shrink-0 items-center justify-center rounded-full border" style={{ borderColor: selected ? "hsl(var(--primary))" : "hsl(var(--surface-4))", background: selected ? "hsl(var(--primary))" : "transparent" }}>
                    {selected && <Check size={11} strokeWidth={3} className="text-white" />}
                  </span>
                  <span className="min-w-0">
                    <span className="flex items-center gap-1.5 text-[13px] font-medium text-foreground">{option.icon}{option.label}</span>
                    <span className="block text-[11px] text-muted-foreground mt-1">{option.description}</span>
                  </span>
                </button>
              );
            })}
          </div>
        ) : (
          <div className="rounded-xl border px-4 py-3 flex items-center justify-between gap-3" style={{ borderColor: "hsl(var(--surface-3))" }}>
            <div>
              <p className="text-[13px] font-medium text-foreground flex items-center gap-1.5"><Cloud size={14} /> Cloud Nemotron</p>
              <p className="text-[11px] text-muted-foreground mt-1">Selected and enforced for live dictation.</p>
            </div>
            <span className="text-[11px] inline-flex items-center gap-1 text-primary"><Check size={13} /> Ready</span>
          </div>
        )}

        {policy.setup_kind === "local_required" && localSelected && recommended && (
          <div className="rounded-xl border px-4 py-3" style={{ borderColor: "hsl(var(--surface-3))" }} aria-live="polite">
            <div className="flex items-center justify-between gap-3">
              <div>
                <p className="text-[13px] font-medium text-foreground">{active?.name ?? recommended.name}</p>
                <p className="text-[11px] text-muted-foreground mt-1">
                  {active && active.key !== recommended.key
                    ? `${active.name} is your retained working model. ${recommended.name} remains recommended.`
                    : recommended.installed
                      ? "Installed and selected for local dictation."
                      : `${recommended.size_hint} download required for local dictation.`}
                </p>
              </div>
              {!recommended.installed ? (
                <button type="button" className="btn-primary shrink-0" disabled={busy || progress !== null} onClick={() => void downloadRecommended()}>
                  {busy || progress !== null ? <Loader2 size={14} className="animate-spin" /> : <Download size={14} />}
                  {progress !== null ? `${progress}%` : "Upgrade"}
                </button>
              ) : active?.key !== recommended.key ? (
                <button type="button" className="btn-primary shrink-0" disabled={busy} onClick={() => void useRecommended()}>
                  {busy ? <Loader2 size={14} className="animate-spin" /> : <Check size={14} />} Use recommended
                </button>
              ) : <span className="text-[11px] inline-flex items-center gap-1 text-primary"><Check size={13} /> Ready</span>}
            </div>
          </div>
        )}

        <div className="rounded-xl border px-4 py-3" style={{ borderColor: "hsl(var(--surface-3))" }}>
          <div className="flex items-start justify-between gap-3">
            <div>
              <p className="text-[13px] font-medium text-foreground flex items-center gap-1.5"><HardDrive size={14} /> Local model storage</p>
              <p className="text-[11px] text-muted-foreground mt-1">
                {installed.length > 0
                  ? installed.map((model) => `${model.name} (${formatSize(model.size_bytes)})`).join(" · ")
                  : "No local speech models are installed."}
              </p>
              {installed.some((model) => model.required_for_meetings) && (
                <p className="text-[11px] text-muted-foreground mt-1">Oriserve is protected during normal cleanup because local Meetings use it.</p>
              )}
            </div>
            {inventory.reclaimable_bytes > 0 && (
              <button type="button" className="btn-ghost shrink-0" disabled={busy} onClick={() => void removeUnused()}>
                {busy ? <Loader2 size={13} className="animate-spin" /> : <Trash2 size={13} />}
                Free {formatSize(inventory.reclaimable_bytes)}
              </button>
            )}
          </div>

          <div className="mt-3 pt-3 border-t" style={{ borderColor: "hsl(var(--surface-3))" }}>
            {confirmDeleteAll ? (
              <div role="alertdialog" aria-labelledby="delete-models-title">
                <p id="delete-models-title" className="text-[12px] font-medium text-foreground">Delete every local speech model?</p>
                <p className="text-[11px] text-muted-foreground mt-1">Dictation will switch to Cloud Nemotron. Local Meetings will require Oriserve to be downloaded again.</p>
                <div className="flex justify-end gap-2 mt-3">
                  <button type="button" autoFocus className="btn-ghost" disabled={busy} onClick={() => setConfirmDeleteAll(false)}>Cancel</button>
                  <button type="button" className="btn-ghost text-destructive" disabled={busy} onClick={() => void deleteAll()}>
                    {busy ? <Loader2 size={13} className="animate-spin" /> : <Trash2 size={13} />} Delete all models
                  </button>
                </div>
              </div>
            ) : (
              <button type="button" className="text-[11px] text-destructive hover:underline" disabled={busy || installed.length === 0} onClick={() => setConfirmDeleteAll(true)}>
                Delete all local speech models
              </button>
            )}
          </div>
        </div>

        {notice && <p className="text-[11px] text-primary" role="status">{notice}</p>}
        <ErrorNotice error={error} onRetry={() => void refresh()} />
      </div>
    </div>
  );
}
