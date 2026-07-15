import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { Check, Cloud, Cpu, Download, Loader2 } from "lucide-react";
import type { Preferences } from "../types";
import {
  getDesktopPrefs,
  getSttSetupPolicy,
  invoke,
  selectLocalDictationRoute,
  setDesktopPrefs,
  type SttSetupPolicy,
} from "../lib/invoke";
import { ErrorNotice } from "./ErrorNotice";
import { friendlyError } from "../lib/friendlyError";

interface ModelStatus {
  installed: boolean;
  size_bytes: number;
  path: string;
}

interface DownloadProgress {
  name: string;
  received: number;
  total: number;
  status: "downloading" | "done" | "error" | string;
  error: string | null;
}

interface DictationSttSectionProps {
  prefs: Preferences | null;
  onPrefsUpdated: (prefs: Preferences) => void;
  platform: string;
}

function localModelCommand(policy: SttSetupPolicy): {
  status: string;
  download: string;
  args?: Record<string, string>;
  event: string;
  eventName: string;
} {
  if (policy.local_model === "nemotron-q4") {
    return {
      status: "nemotron_model_status",
      download: "download_nemotron_model",
      args: { variant: "q4" },
      event: "nemotron-model-download",
      eventName: "nemotron-3.5-asr-streaming-0.6b-Q4_K_M.gguf",
    };
  }
  return {
    status: "dictation_model_status",
    download: "download_dictation_model",
    event: "meeting-model-download",
    eventName: "ggml-oriserve-hinglish-fp16.bin",
  };
}

/**
 * Apple-Silicon-only speech settings. The device policy chooses the local
 * model; users can only opt into the live Cloud Nemotron route. Windows and
 * Intel Macs intentionally render no controls here because their route is
 * fixed at runtime.
 */
export function DictationSttSection({ prefs: _prefs, onPrefsUpdated: _onPrefsUpdated, platform: _platform }: DictationSttSectionProps) {
  const [policy, setPolicy] = useState<SttSetupPolicy | null>(null);
  const [desktopPrefs, setDesktopPrefsState] = useState<Awaited<ReturnType<typeof getDesktopPrefs>> | null>(null);
  const [model, setModel] = useState<ModelStatus | null>(null);
  const [download, setDownload] = useState<DownloadProgress | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const mounted = useRef(true);

  const refresh = useCallback(async () => {
    try {
      const nextPolicy = await getSttSetupPolicy();
      const nextPrefs = await getDesktopPrefs();
      if (!mounted.current) return;
      setPolicy(nextPolicy);
      setDesktopPrefsState(nextPrefs);
      if (nextPolicy.setup_kind === "local_required") {
        const command = localModelCommand(nextPolicy);
        const status = await invoke<ModelStatus>(command.status, command.args);
        if (mounted.current) setModel(status);
      }
    } catch (cause) {
      if (mounted.current) setError(friendlyError(cause));
    }
  }, []);

  useEffect(() => {
    mounted.current = true;
    void refresh();
    return () => {
      mounted.current = false;
    };
  }, [refresh]);

  useEffect(() => {
    if (!policy || policy.setup_kind !== "local_required") return;
    const command = localModelCommand(policy);
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
    return () => {
      void unlisten.then((stop) => stop());
    };
  }, [policy, refresh]);

  const selectRoute = useCallback(async (route: "local" | "cloud-nemotron-3.5") => {
    if (!desktopPrefs || desktopPrefs.dictation_stt === route) return;
    setError("");
    const next = { ...desktopPrefs, dictation_stt: route };
    setDesktopPrefsState(next);
    try {
      await setDesktopPrefs(next);
      await refresh();
    } catch (cause) {
      setError(friendlyError(cause));
      await refresh();
    }
  }, [desktopPrefs, refresh]);

  const downloadLocal = useCallback(async () => {
    if (!policy) return;
    const command = localModelCommand(policy);
    setBusy(true);
    setError("");
    try {
      await invoke(command.download, command.args);
      const status = await invoke<ModelStatus>(command.status, command.args);
      if (!status.installed) {
        throw new Error(`${policy.local_model_name} did not install correctly. Try again.`);
      }
      const nextPrefs = await selectLocalDictationRoute();
      if (mounted.current) setDesktopPrefsState(nextPrefs);
      await refresh();
    } catch (cause) {
      setError(friendlyError(cause));
    } finally {
      setBusy(false);
    }
  }, [policy, refresh, selectLocalDictationRoute]);

  // Cloud-locked devices must not be offered a control that cannot alter the
  // effective route. This covers Windows and Intel Macs.
  if (!policy || policy.setup_kind === "cloud_locked") return null;

  const localSelected = desktopPrefs?.dictation_stt !== "cloud-nemotron-3.5";
  const progress = download && download.total > 0
    ? Math.min(100, Math.round((download.received / download.total) * 100))
    : null;

  return (
    <div className="panel overflow-hidden mb-7">
      <div className="px-5 py-4 border-b" style={{ borderColor: "hsl(var(--surface-3))" }}>
        <p className="text-[13px] font-medium text-foreground">Speech recognition</p>
        <p className="text-[12px] text-muted-foreground mt-0.5">
          This Mac is configured for {policy.local_model_name ?? "local speech recognition"}.
        </p>
      </div>

      <div className="px-5 py-4 flex flex-col gap-3">
        <div className="rounded-xl border overflow-hidden" style={{ borderColor: "hsl(var(--surface-3))", background: "hsl(var(--surface-2))" }}>
          {[
            {
              id: "local" as const,
              icon: <Cpu size={14} />,
              label: "Local",
              description: `${policy.local_model_name ?? "Recommended local model"} · private and no per-use speech cost`,
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

        {localSelected && (
          <div className="rounded-xl border px-4 py-3" style={{ borderColor: "hsl(var(--surface-3))" }}>
            <div className="flex items-center justify-between gap-3">
              <div>
                <p className="text-[13px] font-medium text-foreground">{policy.local_model_name}</p>
                <p className="text-[11px] text-muted-foreground mt-1">
                  {model?.installed ? "Installed and selected for local dictation." : `${policy.local_model_size_hint} download required for local dictation.`}
                </p>
              </div>
              {model?.installed ? (
                <span className="text-[11px] inline-flex items-center gap-1 text-primary"><Check size={13} /> Ready</span>
              ) : (
                <button type="button" className="btn-primary shrink-0" disabled={busy || progress !== null} onClick={() => void downloadLocal()}>
                  {busy || progress !== null ? <Loader2 size={14} className="animate-spin" /> : <Download size={14} />}
                  {progress !== null ? `${progress}%` : "Download"}
                </button>
              )}
            </div>
            <ErrorNotice error={error} onRetry={() => void downloadLocal()} className="mt-2" />
          </div>
        )}

        {!localSelected && <ErrorNotice error={error} onRetry={() => void refresh()} />}
      </div>
    </div>
  );
}
