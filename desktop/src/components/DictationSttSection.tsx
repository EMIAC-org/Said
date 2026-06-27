import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Check, Cloud, Cpu, Download, Loader2, Trash2 } from "lucide-react";
import type { Preferences, SttRuntimeInfo } from "../types";
import { getSttRuntime } from "../lib/invoke";

type SttProviderChoice = "deepgram" | "whisper_local";

interface DictationModelStatus {
  installed: boolean;
  size_bytes: number;
  path: string;
}

interface MeetingModelProgress {
  name: string;
  received: number;
  total: number;
  status: "downloading" | "done" | "error" | string;
  error: string | null;
}

const DICTATION_MODEL_NAME = "ggml-oriserve-hinglish-fp16.bin";

function formatSize(bytes: number): string {
  if (bytes >= 1e9) return `${(bytes / 1e9).toFixed(1)} GB`;
  if (bytes >= 1e6) return `${Math.round(bytes / 1e6)} MB`;
  if (bytes > 0) return `${Math.round(bytes / 1e3)} KB`;
  return "—";
}

interface DictationSttSectionProps {
  prefs: Preferences | null;
  onPrefsUpdated: (prefs: Preferences) => void;
  platform: string;
}

export function DictationSttSection({ prefs, onPrefsUpdated, platform: _platform }: DictationSttSectionProps) {
  const [runtime, setRuntime] = useState<SttRuntimeInfo | null>(null);
  const [whisperModel, setWhisperModel] = useState<DictationModelStatus | null>(null);
  const [whisperDownload, setWhisperDownload] = useState<MeetingModelProgress | null>(null);
  const [confirmDeleteWhisper, setConfirmDeleteWhisper] = useState(false);
  const [deletingWhisper, setDeletingWhisper] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [successMsg, setSuccessMsg] = useState<string | null>(null);
  const mounted = useRef(true);
  const successTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const whisperInstalled = deletingWhisper
    ? false
    : (whisperModel?.installed ?? runtime?.whisper_installed ?? false);

  // Which option shows as selected. Honor an explicit, still-usable choice;
  // otherwise auto-select the installed-aware effective provider so the picker
  // follows what onboarding actually set up — the downloaded local model if it's
  // present, else Deepgram. (Empty pref or a local choice whose model is no
  // longer installed both fall through to the effective default.)
  const storedProvider = (prefs?.stt_provider ?? "").trim();
  let rawProvider: string;
  if (storedProvider === "deepgram") {
    rawProvider = "deepgram";
  } else if (storedProvider === "whisper_local" && whisperInstalled) {
    rawProvider = "whisper_local";
  } else {
    rawProvider = runtime?.effective_provider || "deepgram";
  }
  const provider: SttProviderChoice =
    rawProvider === "whisper_local" && whisperInstalled ? "whisper_local" : "deepgram";

  const showSuccess = useCallback((msg: string) => {
    if (successTimer.current) clearTimeout(successTimer.current);
    setSuccessMsg(msg);
    successTimer.current = setTimeout(() => {
      if (mounted.current) setSuccessMsg(null);
    }, 3200);
  }, []);

  const refresh = useCallback(async () => {
    try {
      const [rt, wstatus] = await Promise.all([
        getSttRuntime(),
        invoke<DictationModelStatus>("dictation_model_status").catch(() => null),
      ]);
      if (!mounted.current) return;
      setRuntime(rt);
      if (wstatus) setWhisperModel(wstatus);
    } catch (e) {
      if (mounted.current) setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  useEffect(() => {
    mounted.current = true;
    void refresh();
    return () => {
      mounted.current = false;
      if (successTimer.current) clearTimeout(successTimer.current);
    };
  }, [refresh]);

  useEffect(() => {
    const unlisten = listen<MeetingModelProgress>("meeting-model-download", (event) => {
      const p = event.payload;
      if (p.name !== DICTATION_MODEL_NAME) return;
      if (p.status === "downloading") {
        setWhisperDownload(p);
        setError(null);
        setSuccessMsg(null);
      } else {
        setWhisperDownload(null);
      }
      if (p.status === "done") {
        showSuccess("On-device model downloaded");
        void refresh();
      }
      if (p.status === "error" && p.error) setError(p.error);
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [refresh, showSuccess]);

  const downloadWhisperModel = async () => {
    setError(null);
    setSuccessMsg(null);
    try {
      await invoke("download_dictation_model");
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const deleteWhisperModel = async () => {
    setConfirmDeleteWhisper(false);
    setDeletingWhisper(true);
    setError(null);
    try {
      await invoke("delete_dictation_model");
      showSuccess("On-device model removed");
      await refresh();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setDeletingWhisper(false);
    }
  };

  const selectProvider = async (next: SttProviderChoice) => {
    if (next === provider) return;
    if (next === "whisper_local" && !whisperInstalled && !deletingWhisper) {
      setError("Download the on-device model first, then select Local.");
      setSuccessMsg(null);
      return;
    }
    setBusy(true);
    setError(null);
    setSuccessMsg(null);
    const prevProvider = provider;
    if (prefs) {
      onPrefsUpdated({ ...prefs, stt_provider: next });
    }
    try {
      const updated = await invoke<Preferences>("patch_preferences", {
        update: { stt_provider: next },
      });
      if (!updated) {
        throw new Error("Failed to save preference");
      }
      onPrefsUpdated(updated);
      await refresh();
    } catch (e) {
      if (prefs) {
        onPrefsUpdated({ ...prefs, stt_provider: prevProvider });
      }
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const whisperDownloadPct =
    whisperDownload && whisperDownload.total > 0
      ? Math.min(100, Math.round((whisperDownload.received / whisperDownload.total) * 100))
      : null;

  return (
    <div className="panel overflow-hidden mb-7">
      <div className="px-5 py-4 border-b" style={{ borderColor: "hsl(var(--surface-3))" }}>
        <p className="text-[13px] font-medium text-foreground">Speech recognition</p>
        <p className="text-[12px] text-muted-foreground mt-0.5">
          Use cloud Deepgram or download the on-device model for local speech recognition.
        </p>
      </div>

      <div className="px-5 py-4 flex flex-col gap-3">
        <div className="grid grid-cols-1 sm:grid-cols-2 gap-2">
          <button
            type="button"
            disabled={busy || deletingWhisper || !!whisperDownload}
            onClick={() => void selectProvider("deepgram")}
            className="rounded-xl px-3 py-2.5 text-left border transition-colors"
            style={{
              borderColor:
                provider === "deepgram" ? "hsl(var(--primary))" : "hsl(var(--surface-3))",
              background:
                provider === "deepgram" ? "hsl(var(--surface-3))" : "hsl(var(--surface-2))",
            }}
          >
            <div className="flex items-center gap-2 text-[13px] font-medium text-foreground">
              <Cloud size={14} />
              Cloud
              {provider === "deepgram" && <Check size={14} className="ml-auto text-primary" />}
            </div>
            <p className="text-[11px] text-muted-foreground mt-1">
              {runtime?.deepgram_configured
                ? "Deepgram ready"
                : "Deepgram cloud speech recognition"}
            </p>
          </button>

          <button
            type="button"
            disabled={busy || deletingWhisper || !!whisperDownload}
            onClick={() => void selectProvider("whisper_local")}
            className="rounded-xl px-3 py-2.5 text-left border transition-colors disabled:opacity-50"
            style={{
              borderColor:
                provider === "whisper_local" ? "hsl(var(--primary))" : "hsl(var(--surface-3))",
              background:
                provider === "whisper_local" ? "hsl(var(--surface-3))" : "hsl(var(--surface-2))",
            }}
          >
            <div className="flex items-center gap-2 text-[13px] font-medium text-foreground">
              <Cpu size={14} />
              Local
              {provider === "whisper_local" && (
                <Check size={14} className="ml-auto text-primary" />
              )}
            </div>
            <p className="text-[11px] text-muted-foreground mt-1">
              On-device speech recognition · no cloud
            </p>
          </button>
        </div>

        <div className="rounded-xl px-4 py-3" style={{ background: "hsl(var(--surface-2))" }}>
          <div className="flex items-center justify-between gap-3">
            <div className="min-w-0">
              <p className="text-[12px] font-medium text-foreground">On-device model</p>
              <p className="text-[11px] text-muted-foreground">
                {deletingWhisper
                  ? "Removing…"
                  : whisperModel?.installed
                    ? `Installed · ${formatSize(whisperModel.size_bytes)}`
                    : whisperDownload
                      ? `Downloading… ${whisperDownloadPct ?? 0}%`
                      : "Download once to use local speech recognition"}
              </p>
            </div>
            <div className="flex items-center gap-2 shrink-0">
              {deletingWhisper ? (
                <span className="flex items-center gap-1.5 text-[11px] text-muted-foreground">
                  <Loader2 size={12} className="animate-spin" /> Removing…
                </span>
              ) : whisperModel?.installed ? (
                confirmDeleteWhisper ? (
                  <div className="flex items-center gap-1.5">
                    <button
                      type="button"
                      onClick={() => void deleteWhisperModel()}
                      className="rounded-lg px-2 py-1 text-[11px] font-medium text-white"
                      style={{ background: "hsl(0 72% 51%)" }}
                    >
                      Delete
                    </button>
                    <button
                      type="button"
                      onClick={() => setConfirmDeleteWhisper(false)}
                      className="rounded-lg px-2 py-1 text-[11px] text-muted-foreground"
                    >
                      Cancel
                    </button>
                  </div>
                ) : (
                  <div className="flex items-center gap-2">
                    <span className="flex items-center gap-1.5 text-[11px] text-primary">
                      <Check size={12} /> Ready
                    </span>
                    <button
                      type="button"
                      onClick={() => setConfirmDeleteWhisper(true)}
                      className="text-muted-foreground hover:text-foreground transition-colors"
                      title="Remove on-device model"
                    >
                      <Trash2 size={13} />
                    </button>
                  </div>
                )
              ) : whisperDownload ? (
                <span className="flex items-center gap-1.5 text-[11px] text-muted-foreground">
                  <Loader2 size={12} className="animate-spin" /> {whisperDownloadPct ?? 0}%
                </span>
              ) : (
                <button
                  type="button"
                  onClick={() => void downloadWhisperModel()}
                  className="flex items-center gap-1.5 rounded-lg px-2.5 py-1.5 text-[11px] font-medium"
                  style={{
                    background: "hsl(var(--primary))",
                    color: "hsl(var(--primary-foreground))",
                  }}
                >
                  <Download size={12} /> Download
                </button>
              )}
            </div>
          </div>
          {whisperDownload && whisperDownloadPct !== null && (
            <div
              className="mt-2 h-1.5 rounded-full overflow-hidden"
              style={{ background: "hsl(var(--surface-4))" }}
            >
              <div
                className="h-full rounded-full transition-all"
                style={{ width: `${whisperDownloadPct}%`, background: "hsl(var(--primary))" }}
              />
            </div>
          )}
        </div>

        {successMsg && (
          <p className="text-[11px] text-emerald-600">{successMsg}</p>
        )}
        {error && (
          <p className="text-[11px] text-red-500">{error}</p>
        )}
      </div>
    </div>
  );
}
