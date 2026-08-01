import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Check, Cloud, Cpu, HardDrive, Loader2, Trash2 } from "lucide-react";
import type { Preferences } from "../types";
import {
  chooseInstalledLocalModel,
  deleteAllLocalSpeechModels,
  getDesktopPrefs,
  getLocalModelInventory,
  getLocalAsrRuntimeStatus,
  getSttSetupPolicy,
  removeUnusedLocalDictationModels,
  setDesktopPrefs,
  type LocalModelInventory,
  type LocalModelInfo,
  type LocalModelKey,
  type LocalAsrRuntimeStatus,
  type DictationRoute,
  type SttSetupPolicy,
} from "../lib/invoke";
import {
  cancelLocalModelDownload,
  isCancelledDownload,
  startLocalModelDownload,
  useLocalModelDownloads,
} from "../lib/localModels";
import { ErrorNotice } from "./ErrorNotice";
import { LocalModelRow } from "./LocalModelRow";
import { friendlyError } from "../lib/friendlyError";
import { dictationRouteOptions } from "../lib/dictationCatalogue";

interface DictationSttSectionProps {
  prefs: Preferences | null;
  onPrefsUpdated: (prefs: Preferences) => void;
  platform: string;
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
  const [runtime, setRuntime] = useState<LocalAsrRuntimeStatus | null>(null);
  // Which model the user is currently acting on. Scoped to one model so a
  // second model's row never renders this model's spinner.
  const [acting, setActing] = useState<LocalModelKey | null>(null);
  const [maintaining, setMaintaining] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [confirmDeleteAll, setConfirmDeleteAll] = useState(false);
  const mounted = useRef(true);

  const refresh = useCallback(async () => {
    try {
      const [nextPolicy, nextPrefs, nextInventory, nextRuntime] = await Promise.all([
        getSttSetupPolicy(),
        getDesktopPrefs(),
        getLocalModelInventory(),
        getLocalAsrRuntimeStatus(),
      ]);
      if (!mounted.current) return;
      setPolicy(nextPolicy);
      setDesktopPrefsState(nextPrefs);
      setInventory(nextInventory);
      setRuntime(nextRuntime);
    } catch (cause) {
      if (mounted.current) setError(friendlyError(cause));
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

  const selectRoute = useCallback(async (route: DictationRoute) => {
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

  const setPolishEnabled = useCallback(async (enabled: boolean) => {
    if (!desktopPrefs || desktopPrefs.polish_enabled === enabled) return;
    setError("");
    const next = { ...desktopPrefs, polish_enabled: enabled };
    setDesktopPrefsState(next);
    try {
      await setDesktopPrefs(next);
      setNotice(enabled
        ? "Polish is enabled for normal dictation."
        : "Polish is off. AirNote will paste the speech transcript unchanged.");
    } catch (cause) {
      setDesktopPrefsState(desktopPrefs);
      setError(friendlyError(cause));
    }
  }, [desktopPrefs]);

  const installModel = useCallback(async (model: LocalModelInfo) => {
    setActing(model.key);
    setError("");
    setNotice("");
    try {
      await startLocalModelDownload(model.key);
      await chooseInstalledLocalModel(model.key);
      setNotice(`${model.name} is installed and selected for local dictation.`);
    } catch (cause) {
      const message = friendlyError(cause);
      if (!isCancelledDownload(message)) setError(message);
    } finally {
      if (mounted.current) setActing(null);
      await refresh();
    }
  }, [refresh]);

  const useModel = useCallback(async (model: LocalModelInfo) => {
    setActing(model.key);
    setError("");
    setNotice("");
    try {
      await chooseInstalledLocalModel(model.key);
      setNotice(`${model.name} is now used for local dictation.`);
    } catch (cause) {
      setError(friendlyError(cause));
    } finally {
      if (mounted.current) setActing(null);
      await refresh();
    }
  }, [refresh]);

  const cancelModel = useCallback(async (model: LocalModelInfo) => {
    await cancelLocalModelDownload(model.key);
  }, []);

  const removeUnused = useCallback(async () => {
    setMaintaining(true);
    setError("");
    setNotice("");
    try {
      const result = await removeUnusedLocalDictationModels();
      setNotice(result.removed.length > 0
        ? `Removed ${result.removed.map((model) => model.name).join(", ")} and freed ${formatSize(result.freed_bytes)}.`
        : "No unused local dictation models were found.");
    } catch (cause) {
      setError(friendlyError(cause));
    } finally {
      if (mounted.current) setMaintaining(false);
      await refresh();
    }
  }, [refresh]);

  const deleteAll = useCallback(async () => {
    setMaintaining(true);
    setError("");
    setNotice("");
    try {
      const result = await deleteAllLocalSpeechModels();
      setNotice(result.removed.length > 0
        ? `Deleted all local speech models and freed ${formatSize(result.freed_bytes)}.`
        : "No local speech models were installed.");
      setConfirmDeleteAll(false);
    } catch (cause) {
      setError(friendlyError(cause));
    } finally {
      if (mounted.current) setMaintaining(false);
      await refresh();
    }
  }, [refresh]);

  // Recommended first, otherwise catalog order. One list, one row per model —
  // the recommended model is a badge, not a second UI with its own progress.
  const selectableModels = useMemo(() => {
    const models = (inventory?.models ?? []).filter((model) => model.selectable);
    return [...models].sort((left, right) => Number(right.recommended) - Number(left.recommended));
  }, [inventory?.models]);

  if (!policy || !inventory || !desktopPrefs) return null;

  const active = inventory.models.find((model) => model.active_for_dictation);
  const installed = inventory.models.filter((model) => model.installed);
  const busy = acting !== null || maintaining;

  return (
    <div className="panel overflow-hidden mb-7">
      <div className="px-5 py-4 border-b" style={{ borderColor: "hsl(var(--surface-3))" }}>
        <p className="text-[13px] font-medium text-foreground">Speech recognition</p>
        <p className="text-[12px] text-muted-foreground mt-0.5">
          {policy.setup_kind === "cloud_locked"
            ? "Choose a cloud model for dictation on this device. Local files are used only by Meetings."
            : `This Mac recommends ${policy.local_model_name ?? "local speech recognition"}.`}
        </p>
      </div>

      <div className="px-5 py-4 flex flex-col gap-3">
        <div className="rounded-xl border overflow-hidden" style={{ borderColor: "hsl(var(--surface-3))", background: "hsl(var(--surface-2))" }}>
          {dictationRouteOptions(policy).map((option, index) => {
            const selected = option.id === desktopPrefs.dictation_stt;
            const Icon = option.kind === "local" ? Cpu : Cloud;
            const label = option.kind === "local"
              ? `Local · ${active?.name ?? option.label}`
              : `Cloud · ${option.label}`;
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
                  <span className="flex items-center gap-1.5 text-[13px] font-medium text-foreground"><Icon size={14} />{label}</span>
                  <span className="block text-[11px] text-muted-foreground mt-1">{option.description}</span>
                  <span className="block text-[10px] text-muted-foreground mt-1">{option.provider} · {option.detail}</span>
                </span>
              </button>
            );
          })}
        </div>

        <div className="rounded-xl border px-4 py-3 flex items-center justify-between gap-3" style={{ borderColor: "hsl(var(--surface-3))" }}>
          <div>
            <p className="text-[13px] font-medium text-foreground">Polish transcription</p>
            <p className="text-[11px] text-muted-foreground mt-1">
              {desktopPrefs.polish_enabled
                ? "Improve wording and formatting after speech recognition."
                : "Paste the speech model's final transcript directly; works fully offline with a local model."}
            </p>
          </div>
          <button
            type="button"
            role="switch"
            aria-checked={desktopPrefs.polish_enabled}
            aria-label="Polish transcription"
            onClick={() => void setPolishEnabled(!desktopPrefs.polish_enabled)}
            className="relative h-6 w-11 shrink-0 rounded-full transition-colors"
            style={{ background: desktopPrefs.polish_enabled ? "hsl(var(--primary))" : "hsl(var(--surface-4))" }}
          >
            <span className="absolute top-0.5 h-5 w-5 rounded-full bg-white shadow transition-transform" style={{ left: 2, transform: desktopPrefs.polish_enabled ? "translateX(20px)" : "translateX(0)" }} />
          </button>
        </div>

        {selectableModels.length > 0 && (
          <div className="rounded-xl border overflow-hidden" style={{ borderColor: "hsl(var(--surface-3))" }}>
            <div className="px-4 py-3 border-b" style={{ borderColor: "hsl(var(--surface-3))" }}>
              <p className="text-[13px] font-medium text-foreground">Local speech models</p>
              <p className="text-[11px] text-muted-foreground mt-1">
                Install and compare models that run on this Mac. Using one switches Dictation to local; AirNote's cloud and meeting models are unaffected.
              </p>
            </div>
            {selectableModels.map((model, index) => (
              <div
                key={model.key}
                style={index > 0 ? { borderTop: "1px solid hsl(var(--surface-3))" } : undefined}
              >
                <LocalModelRow
                  model={model}
                  download={downloads[model.key]}
                  pending={acting === model.key}
                  locked={busy && acting !== model.key}
                  onInstall={(target) => void installModel(target)}
                  onUse={(target) => void useModel(target)}
                  onCancel={(target) => void cancelModel(target)}
                />
              </div>
            ))}
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
              {runtime?.loaded_model && (
                <p className="text-[11px] text-muted-foreground mt-1">
                  Loaded: {runtime.loaded_model} · {runtime.backend ?? "automatic backend"}{runtime.supports_streaming ? " · streaming capable" : " · batch"}{runtime.last_load_ms !== null ? ` · ${runtime.last_load_ms} ms load` : ""}
                </p>
              )}
              {runtime?.last_error && <p className="text-[11px] text-destructive mt-1">Last local runtime error: {runtime.last_error}</p>}
            </div>
            {inventory.reclaimable_bytes > 0 && (
              <button type="button" className="btn-ghost shrink-0" disabled={busy} onClick={() => void removeUnused()}>
                {maintaining ? <Loader2 size={13} className="animate-spin" /> : <Trash2 size={13} />}
                Free {formatSize(inventory.reclaimable_bytes)}
              </button>
            )}
          </div>

          <div className="mt-3 pt-3 border-t" style={{ borderColor: "hsl(var(--surface-3))" }}>
            {confirmDeleteAll ? (
              <div role="alertdialog" aria-labelledby="delete-models-title">
                <p id="delete-models-title" className="text-[12px] font-medium text-foreground">Delete every local speech model?</p>
                <p className="text-[11px] text-muted-foreground mt-1">Dictation will switch to a cloud speech model. Local Meetings will require Oriserve to be downloaded again.</p>
                <div className="flex justify-end gap-2 mt-3">
                  <button type="button" autoFocus className="btn-ghost" disabled={busy} onClick={() => setConfirmDeleteAll(false)}>Cancel</button>
                  <button type="button" className="btn-ghost text-destructive" disabled={busy} onClick={() => void deleteAll()}>
                    {maintaining ? <Loader2 size={13} className="animate-spin" /> : <Trash2 size={13} />} Delete all models
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
