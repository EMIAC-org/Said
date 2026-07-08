import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Check, Cpu, Download, Loader2, Trash2 } from "lucide-react";
import type { Preferences, SttRuntimeInfo } from "../types";
import { getSttRuntime } from "../lib/invoke";
import { NEW_MODEL_FILE, NEW_MODEL_NAME, NEW_MODEL_SIZE_HINT } from "../lib/onDeviceModel";
import { ReclaimOldModelsRow, type ReclaimResult } from "./ReclaimOldModelsRow";
import { friendlyError } from "../lib/friendlyError";
import { ErrorNotice } from "./ErrorNotice";

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

export function DictationSttSection({ prefs: _prefs, onPrefsUpdated: _onPrefsUpdated, platform: _platform }: DictationSttSectionProps) {
  const [runtime, setRuntime] = useState<SttRuntimeInfo | null>(null);
  const [whisperModel, setWhisperModel] = useState<DictationModelStatus | null>(null);
  const [whisperDownload, setWhisperDownload] = useState<MeetingModelProgress | null>(null);
  const [confirmDeleteWhisper, setConfirmDeleteWhisper] = useState(false);
  const [deletingWhisper, setDeletingWhisper] = useState(false);
  const [repairing, setRepairing] = useState(false);
  const [reclaiming, setReclaiming] = useState(false);
  const [reclaimResult, setReclaimResult] = useState<ReclaimResult | null>(null);
  const [reclaimError, setReclaimError] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [successMsg, setSuccessMsg] = useState<string | null>(null);
  const mounted = useRef(true);
  const successTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

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
      if (p.name !== NEW_MODEL_FILE) return;
      if (p.status === "downloading") {
        setWhisperDownload(p);
        setError(null);
        setSuccessMsg(null);
      } else {
        setWhisperDownload(null);
      }
      if (p.status === "done") {
        showSuccess(`${NEW_MODEL_NAME} downloaded`);
        void refresh();
      }
      if (p.status === "error" && p.error) setError(friendlyError(p.error));
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
      setError(friendlyError(e));
    }
  };

  const deleteWhisperModel = async () => {
    setConfirmDeleteWhisper(false);
    setDeletingWhisper(true);
    setError(null);
    try {
      await invoke("delete_dictation_model");
      showSuccess(`${NEW_MODEL_NAME} removed`);
      await refresh();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setDeletingWhisper(false);
    }
  };

  // Repair: delete the model file and re-download it. The one-click recovery
  // when a model is present but corrupt (e.g. the SHA-256 check would reject it,
  // or whisper-cli fails to load it). Re-download re-verifies integrity.
  const repairModel = async () => {
    setRepairing(true);
    setError(null);
    setSuccessMsg(null);
    setReclaimResult(null);
    try {
      await invoke("delete_dictation_model");
      setWhisperModel((m) => (m ? { ...m, installed: false } : m));
      await invoke("download_dictation_model");
      await refresh();
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      if (msg !== "cancelled") setError(friendlyError(msg));
    } finally {
      setRepairing(false);
    }
  };

  // Reclaim disk from unsupported extra speech models. The Oriserve model and
  // Silero VAD support model are preserved.
  const reclaimOldModels = async () => {
    setReclaiming(true);
    setReclaimError("");
    try {
      const result = await invoke<ReclaimResult>("reclaim_old_models");
      setReclaimResult(result);
    } catch (e) {
      setReclaimError(friendlyError(e));
    } finally {
      setReclaiming(false);
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
          Dictation uses the local whisper.cpp model on this device.
        </p>
      </div>

      <div className="px-5 py-4 flex flex-col gap-3">
        <div
          className="rounded-xl px-3 py-2.5 border"
          style={{ borderColor: "hsl(var(--surface-3))", background: "hsl(var(--surface-2))" }}
        >
          <div className="flex items-center gap-2 text-[13px] font-medium text-foreground">
            <Cpu size={14} />
            Local speech engine
            {runtime?.whisper_ready && <Check size={14} className="ml-auto text-primary" />}
          </div>
          <p className="text-[11px] text-muted-foreground mt-1">
            {runtime?.whisper_ready
              ? "Ready for dictation and meetings"
              : "Local speech model required before dictation can run"}
          </p>
        </div>

        <div className="rounded-xl px-4 py-3" style={{ background: "hsl(var(--surface-2))" }}>
          <div className="flex items-center justify-between gap-3">
            <div className="min-w-0">
              <div className="flex items-center gap-1.5">
                <p className="text-[12px] font-medium text-foreground">{NEW_MODEL_NAME}</p>
                <span
                  className="text-[9px] px-1.5 py-px rounded-full font-semibold uppercase tracking-wide"
                  style={{ background: "hsl(var(--primary) / 0.18)", color: "hsl(var(--primary))" }}
                >
                  Local
                </span>
              </div>
              <p className="text-[11px] text-muted-foreground">
                {deletingWhisper
                  ? "Removing…"
                  : whisperModel?.installed
                    ? `Installed · ${formatSize(whisperModel.size_bytes)}`
                    : whisperDownload
                      ? `Downloading… ${whisperDownloadPct ?? 0}%`
                      : `On-device Hinglish model · ${NEW_MODEL_SIZE_HINT}`}
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
                  <div className="flex items-center gap-2.5">
                    <span className="flex items-center gap-1.5 text-[11px] text-primary">
                      <Check size={12} /> Ready
                    </span>
                    <button
                      type="button"
                      onClick={() => void repairModel()}
                      disabled={repairing}
                      className="text-[11px] text-muted-foreground hover:text-foreground transition-colors disabled:opacity-50"
                      title="Delete and re-download the model (fixes a corrupt file)"
                    >
                      {repairing ? "Repairing…" : "Repair"}
                    </button>
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

          {/* Reclaim extra speech-model disk once Oriserve is installed. */}
          {whisperModel?.installed && !repairing && (
            <ReclaimOldModelsRow
              reclaiming={reclaiming}
              result={reclaimResult}
              error={reclaimError}
              onReclaim={() => void reclaimOldModels()}
            />
          )}
        </div>

        {successMsg && (
          <p className="text-[11px] text-emerald-600">{successMsg}</p>
        )}
        <ErrorNotice error={error} onRetry={() => void downloadWhisperModel()} />
      </div>
    </div>
  );
}
