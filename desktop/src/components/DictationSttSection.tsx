import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Check, Cloud, Cpu, Download, Loader2, Trash2 } from "lucide-react";
import type { Preferences, SttRuntimeInfo } from "../types";
import { getDesktopPrefs, getSttRuntime, setDesktopPrefs, type DesktopPrefs } from "../lib/invoke";
import { NEW_MODEL_FILE, NEW_MODEL_NAME, NEW_MODEL_SIZE_HINT } from "../lib/onDeviceModel";
import { NEMOTRON_MODEL_FILE, NEMOTRON_MODEL_NAME, NEMOTRON_MODEL_SIZE_HINT } from "../lib/nemotronModel";
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

export function DictationSttSection({ prefs: _prefs, onPrefsUpdated: _onPrefsUpdated, platform }: DictationSttSectionProps) {
  // Windows dictation is provider-selectable (see dictation_stt.rs): Auto
  // (capability-routed), On-device, or Cloud. macOS is always on-device.
  // The on-device whisper model still powers meetings on every platform.
  const selectableProvider = platform === "windows";
  const [runtime, setRuntime] = useState<SttRuntimeInfo | null>(null);
  const [desktopPrefs, setDesktopPrefsState] = useState<DesktopPrefs | null>(null);
  const [whisperModel, setWhisperModel] = useState<DictationModelStatus | null>(null);
  const [whisperDownload, setWhisperDownload] = useState<MeetingModelProgress | null>(null);
  const [nemotronModel, setNemotronModel] = useState<DictationModelStatus | null>(null);
  const [nemotronDownload, setNemotronDownload] = useState<MeetingModelProgress | null>(null);
  const [confirmDeleteNemotron, setConfirmDeleteNemotron] = useState(false);
  const [deletingNemotron, setDeletingNemotron] = useState(false);
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
      const [rt, dp, wstatus, nstatus] = await Promise.all([
        getSttRuntime(),
        getDesktopPrefs().catch(() => null),
        invoke<DictationModelStatus>("dictation_model_status").catch(() => null),
        invoke<DictationModelStatus>("nemotron_model_status").catch(() => null),
      ]);
      if (!mounted.current) return;
      setRuntime(rt);
      if (dp) setDesktopPrefsState(dp);
      if (wstatus) setWhisperModel(wstatus);
      if (nstatus) setNemotronModel(nstatus);
    } catch (e) {
      if (mounted.current) setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  // Switch the dictation provider; applies to the very next dictation.
  const selectProvider = useCallback(
    async (choice: DesktopPrefs["dictation_stt"]) => {
      if (!desktopPrefs || desktopPrefs.dictation_stt === choice) return;
      const next = { ...desktopPrefs, dictation_stt: choice };
      setDesktopPrefsState(next);
      try {
        await setDesktopPrefs(next);
        await refresh();
      } catch (e) {
        setError(friendlyError(e));
      }
    },
    [desktopPrefs, refresh],
  );

  const selectLocalModel = useCallback(
    async (choice: DesktopPrefs["local_stt_model"]) => {
      if (!desktopPrefs || desktopPrefs.local_stt_model === choice) return;
      if (choice === "nemotron" && !nemotronModel?.installed) {
        setError("Download Nemotron before selecting it.");
        return;
      }
      const next = { ...desktopPrefs, local_stt_model: choice };
      setDesktopPrefsState(next);
      try {
        await setDesktopPrefs(next);
        await refresh();
      } catch (e) {
        setError(friendlyError(e));
      }
    },
    [desktopPrefs, nemotronModel?.installed, refresh],
  );

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

  useEffect(() => {
    const unlisten = listen<MeetingModelProgress>("nemotron-model-download", (event) => {
      const p = event.payload;
      if (p.name !== NEMOTRON_MODEL_FILE) return;
      if (p.status === "downloading") {
        setNemotronDownload(p);
        setError(null);
        setSuccessMsg(null);
      } else {
        setNemotronDownload(null);
      }
      if (p.status === "done") {
        showSuccess(`${NEMOTRON_MODEL_NAME} downloaded`);
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

  const downloadNemotronModel = async () => {
    setError(null);
    setSuccessMsg(null);
    try {
      await invoke("download_nemotron_model");
    } catch (e) {
      setError(friendlyError(e));
    }
  };

  const deleteNemotronModel = async () => {
    setConfirmDeleteNemotron(false);
    setDeletingNemotron(true);
    setError(null);
    try {
      // Never leave the next dictation pointing at a file we are about to
      // remove. Oriserve remains the durable default and Meetings use it too.
      if (desktopPrefs?.local_stt_model === "nemotron") {
        const next = { ...desktopPrefs, local_stt_model: "oriserve" as const };
        await setDesktopPrefs(next);
        setDesktopPrefsState(next);
      }
      await invoke("delete_nemotron_model");
      showSuccess(`${NEMOTRON_MODEL_NAME} removed`);
      await refresh();
    } catch (e) {
      setError(friendlyError(e));
    } finally {
      setDeletingNemotron(false);
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
  const nemotronDownloadPct =
    nemotronDownload && nemotronDownload.total > 0
      ? Math.min(100, Math.round((nemotronDownload.received / nemotronDownload.total) * 100))
      : null;

  return (
    <div className="panel overflow-hidden mb-7">
      <div className="px-5 py-4 border-b" style={{ borderColor: "hsl(var(--surface-3))" }}>
        <p className="text-[13px] font-medium text-foreground">Speech recognition</p>
        <p className="text-[12px] text-muted-foreground mt-0.5">
          {selectableProvider
            ? "Choose how dictation is transcribed on this device. Meetings always use the on-device model."
            : "Choose the local speech model used for dictation on this device."}
        </p>
      </div>

      <div className="px-5 py-4 flex flex-col gap-3">
        {selectableProvider ? (
          <div
            className="rounded-xl border overflow-hidden"
            style={{ borderColor: "hsl(var(--surface-3))", background: "hsl(var(--surface-2))" }}
          >
            {(
              [
                {
                  id: "auto" as const,
                  icon: <Check size={14} />,
                  label: "Auto",
                  badge: "Recommended",
                  desc:
                    runtime?.dictation_auto_provider === "on-device/whisper"
                      ? "Best for this device — uses the on-device engine (GPU detected)"
                      : "Best for this device — uses the cloud engine (no usable GPU found)",
                },
                {
                  id: "local" as const,
                  icon: <Cpu size={14} />,
                  label: "On-device",
                  badge: null,
                  desc: "Private and offline. Fast with a supported GPU; needs the local model",
                },
                {
                  id: "hosted" as const,
                  icon: <Cloud size={14} />,
                  label: "Cloud",
                  badge: null,
                  desc: "Whisper large-v3 on AirNote's speech service. Internet required",
                },
              ]
            ).map((opt, i) => {
              const selected = (desktopPrefs?.dictation_stt ?? "auto") === opt.id;
              return (
                <button
                  key={opt.id}
                  type="button"
                  onClick={() => void selectProvider(opt.id)}
                  className="w-full text-left px-3 py-2.5 flex items-start gap-2.5 transition-colors hover:bg-black/5 dark:hover:bg-white/5"
                  style={i > 0 ? { borderTop: "1px solid hsl(var(--surface-3))" } : undefined}
                >
                  <span
                    className="mt-0.5 flex h-4 w-4 shrink-0 items-center justify-center rounded-full border"
                    style={{
                      borderColor: selected ? "hsl(var(--primary))" : "hsl(var(--surface-4))",
                      background: selected ? "hsl(var(--primary))" : "transparent",
                    }}
                  >
                    {selected && <Check size={11} strokeWidth={3} className="text-white" />}
                  </span>
                  <span className="min-w-0">
                    <span className="flex items-center gap-1.5 text-[13px] font-medium text-foreground">
                      {opt.label}
                      {opt.badge && (
                        <span
                          className="text-[9px] px-1.5 py-px rounded-full font-semibold uppercase tracking-wide"
                          style={{ background: "hsl(var(--primary) / 0.18)", color: "hsl(var(--primary))" }}
                        >
                          {opt.badge}
                        </span>
                      )}
                    </span>
                    <span className="block text-[11px] text-muted-foreground mt-0.5">{opt.desc}</span>
                  </span>
                </button>
              );
            })}
          </div>
        ) : (
          <div
            className="rounded-xl px-3 py-2.5 border"
            style={{ borderColor: "hsl(var(--surface-3))", background: "hsl(var(--surface-2))" }}
          >
            <div className="flex items-center gap-2 text-[13px] font-medium text-foreground">
              <Cpu size={14} />
              Local speech engine
              {runtime?.dictation_ready && <Check size={14} className="ml-auto text-primary" />}
            </div>
            <p className="text-[11px] text-muted-foreground mt-1">
              {runtime?.dictation_ready
                ? runtime?.local_stt_model === "nemotron"
                  ? "Nemotron is ready for dictation; Oriserve remains available for meetings"
                  : "Ready for dictation and meetings"
                : "Selected local speech model is required before dictation can run"}
            </p>
          </div>
        )}

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
                      : selectableProvider
                        ? `Used for meetings and on-device dictation · ${NEW_MODEL_SIZE_HINT}`
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
                    <button
                      type="button"
                      onClick={() => void selectLocalModel("oriserve")}
                      className="flex items-center gap-1.5 rounded-lg px-2.5 py-1.5 text-[11px] font-medium"
                      style={
                        (desktopPrefs?.local_stt_model ?? "oriserve") === "oriserve"
                          ? { background: "hsl(var(--primary) / 0.18)", color: "hsl(var(--primary))" }
                          : { background: "hsl(var(--primary))", color: "hsl(var(--primary-foreground))" }
                      }
                    >
                      {(desktopPrefs?.local_stt_model ?? "oriserve") === "oriserve" ? <Check size={12} /> : null}
                      {(desktopPrefs?.local_stt_model ?? "oriserve") === "oriserve" ? "Selected" : "Use this model"}
                    </button>
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

        <div className="rounded-xl px-4 py-3 border" style={{ borderColor: "hsl(var(--surface-3))", background: "hsl(var(--surface-2))" }}>
          <div className="flex items-center justify-between gap-3">
            <div className="min-w-0">
              <div className="flex items-center gap-1.5">
                <p className="text-[12px] font-medium text-foreground">{NEMOTRON_MODEL_NAME}</p>
                <span
                  className="text-[9px] px-1.5 py-px rounded-full font-semibold uppercase tracking-wide"
                  style={{ background: "hsl(var(--primary) / 0.18)", color: "hsl(var(--primary))" }}
                >
                  Experimental
                </span>
              </div>
              <p className="text-[11px] text-muted-foreground mt-0.5">
                {deletingNemotron
                  ? "Removing…"
                  : nemotronModel?.installed
                    ? `Installed · ${formatSize(nemotronModel.size_bytes)}`
                    : nemotronDownload
                      ? `Downloading… ${nemotronDownloadPct ?? 0}%`
                      : `Optional multilingual local model · ${NEMOTRON_MODEL_SIZE_HINT}`}
              </p>
            </div>
            <div className="flex items-center gap-2 shrink-0">
              {deletingNemotron ? (
                <span className="flex items-center gap-1.5 text-[11px] text-muted-foreground">
                  <Loader2 size={12} className="animate-spin" /> Removing…
                </span>
              ) : nemotronModel?.installed ? (
                confirmDeleteNemotron ? (
                  <div className="flex items-center gap-1.5">
                    <button
                      type="button"
                      onClick={() => void deleteNemotronModel()}
                      className="rounded-lg px-2 py-1 text-[11px] font-medium text-white"
                      style={{ background: "hsl(0 72% 51%)" }}
                    >
                      Delete
                    </button>
                    <button
                      type="button"
                      onClick={() => setConfirmDeleteNemotron(false)}
                      className="rounded-lg px-2 py-1 text-[11px] text-muted-foreground"
                    >
                      Cancel
                    </button>
                  </div>
                ) : (
                  <div className="flex items-center gap-2.5">
                    <button
                      type="button"
                      onClick={() => void selectLocalModel("nemotron")}
                      className="rounded-lg px-2.5 py-1.5 text-[11px] font-medium"
                      style={
                        (desktopPrefs?.local_stt_model ?? "oriserve") === "nemotron"
                          ? { background: "hsl(var(--primary) / 0.18)", color: "hsl(var(--primary))" }
                          : { background: "hsl(var(--primary))", color: "hsl(var(--primary-foreground))" }
                      }
                    >
                      {(desktopPrefs?.local_stt_model ?? "oriserve") === "nemotron" ? "Selected" : "Use this model"}
                    </button>
                    <button
                      type="button"
                      onClick={() => setConfirmDeleteNemotron(true)}
                      className="text-muted-foreground hover:text-foreground transition-colors"
                      title="Remove Nemotron model"
                    >
                      <Trash2 size={13} />
                    </button>
                  </div>
                )
              ) : nemotronDownload ? (
                <span className="flex items-center gap-1.5 text-[11px] text-muted-foreground">
                  <Loader2 size={12} className="animate-spin" /> {nemotronDownloadPct ?? 0}%
                </span>
              ) : (
                <button
                  type="button"
                  onClick={() => void downloadNemotronModel()}
                  className="flex items-center gap-1.5 rounded-lg px-2.5 py-1.5 text-[11px] font-medium"
                  style={{ background: "hsl(var(--primary))", color: "hsl(var(--primary-foreground))" }}
                >
                  <Download size={12} /> Download
                </button>
              )}
            </div>
          </div>
          {nemotronDownload && nemotronDownloadPct !== null && (
            <div className="mt-2 h-1.5 rounded-full overflow-hidden" style={{ background: "hsl(var(--surface-4))" }}>
              <div
                className="h-full rounded-full transition-all"
                style={{ width: `${nemotronDownloadPct}%`, background: "hsl(var(--primary))" }}
              />
            </div>
          )}
          <p className="text-[10px] leading-relaxed text-muted-foreground mt-2">
            Uses a different ASR engine from Whisper. It is opt-in while we validate Hinglish accuracy; Meetings remain on Oriserve.
            {selectableProvider ? " Select On-device above to use your selected local model." : ""}
          </p>
        </div>

        {successMsg && (
          <p className="text-[11px] text-emerald-600">{successMsg}</p>
        )}
        <ErrorNotice error={error} onRetry={() => void downloadWhisperModel()} />
      </div>
    </div>
  );
}
