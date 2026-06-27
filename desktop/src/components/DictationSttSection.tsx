import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Check, Cloud, Cpu, Download, Loader2, Trash2, X } from "lucide-react";
import type { Preferences, SttRuntimeInfo } from "../types";
import { getSttRuntime, patchPreferences } from "../lib/invoke";

type SttProviderChoice = "deepgram" | "swift_local" | "whisper_local";

interface SwiftModelStatus {
  installed: boolean;
  size_bytes: number;
  path: string;
  downloading_percent: number | null;
}

interface SwiftDownloadProgress {
  received: number;
  total: number;
  status: "downloading" | "done" | "cancelled" | "error";
  error: string | null;
}

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

export function DictationSttSection({ prefs, onPrefsUpdated, platform }: DictationSttSectionProps) {
  const isMac = platform === "macos";
  const [runtime, setRuntime] = useState<SttRuntimeInfo | null>(null);
  const [model, setModel] = useState<SwiftModelStatus | null>(null);
  const [swiftDownload, setSwiftDownload] = useState<SwiftDownloadProgress | null>(null);
  const [whisperModel, setWhisperModel] = useState<DictationModelStatus | null>(null);
  const [whisperDownload, setWhisperDownload] = useState<MeetingModelProgress | null>(null);
  const [confirmDeleteWhisper, setConfirmDeleteWhisper] = useState(false);
  const [deletingWhisper, setDeletingWhisper] = useState(false);
  const [busy, setBusy] = useState(false);
  const [deletingSwift, setDeletingSwift] = useState(false);
  const [confirmDeleteSwift, setConfirmDeleteSwift] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [successMsg, setSuccessMsg] = useState<string | null>(null);
  const mounted = useRef(true);
  const successTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const swiftInstalled = deletingSwift ? false : (model?.installed ?? false);
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
  } else if (storedProvider === "swift_local" && isMac && swiftInstalled) {
    rawProvider = "swift_local";
  } else if (storedProvider === "whisper_local" && whisperInstalled) {
    rawProvider = "whisper_local";
  } else {
    rawProvider = runtime?.effective_provider || "deepgram";
  }
  const provider: SttProviderChoice =
    rawProvider === "deepgram"
      ? "deepgram"
      : rawProvider === "swift_local" && isMac
        ? "swift_local"
        : "whisper_local";
  const swiftSelected = provider === "swift_local";

  const showSuccess = useCallback((msg: string) => {
    if (successTimer.current) clearTimeout(successTimer.current);
    setSuccessMsg(msg);
    successTimer.current = setTimeout(() => {
      if (mounted.current) setSuccessMsg(null);
    }, 3200);
  }, []);

  const refresh = useCallback(async () => {
    try {
      const [rt, status, wstatus] = await Promise.all([
        getSttRuntime(),
        isMac
          ? invoke<SwiftModelStatus>("swift_stt_model_status").catch(() => null)
          : Promise.resolve(null),
        invoke<DictationModelStatus>("dictation_model_status").catch(() => null),
      ]);
      if (!mounted.current) return;
      setRuntime(rt);
      if (status) setModel(status);
      if (wstatus) setWhisperModel(wstatus);
    } catch (e) {
      if (mounted.current) setError(e instanceof Error ? e.message : String(e));
    }
  }, [isMac]);

  useEffect(() => {
    mounted.current = true;
    void refresh();
    return () => {
      mounted.current = false;
      if (successTimer.current) clearTimeout(successTimer.current);
    };
  }, [refresh]);

  useEffect(() => {
    const unlistenP = listen<SwiftDownloadProgress>("swift-model-download", (event) => {
      const p = event.payload;
      if (p.status === "downloading") {
        setSwiftDownload(p);
        setError(null);
        setSuccessMsg(null);
      } else {
        setSwiftDownload(null);
      }
      if (p.status === "done") {
        showSuccess("Swift model downloaded");
        void refresh();
      }
      if (p.status === "error" && p.error) setError(p.error);
    });
    return () => {
      void unlistenP.then((fn) => fn());
    };
  }, [refresh, showSuccess]);

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
    if (next === "swift_local" && !swiftInstalled && !deletingSwift) {
      setError("Download the Swift model below, then select Local Swift.");
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

  const startSwiftDownload = async () => {
    setBusy(true);
    setError(null);
    setSuccessMsg(null);
    try {
      await invoke("swift_stt_download_model");
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      if (msg !== "cancelled") setError(msg);
    } finally {
      setBusy(false);
    }
  };

  const cancelSwiftDownload = async () => {
    await invoke("swift_stt_cancel_download").catch(() => {});
    setSwiftDownload(null);
  };

  const deleteSwiftModel = async () => {
    setConfirmDeleteSwift(false);
    setDeletingSwift(true);
    setError(null);
    setSuccessMsg(null);

    const modelSnapshot = model;
    const runtimeSnapshot = runtime;
    const prefsSnapshot = prefs;

    setModel({
      installed: false,
      size_bytes: 0,
      path: modelSnapshot?.path ?? "",
      downloading_percent: null,
    });
    setRuntime((prev) =>
      prev
        ? {
            ...prev,
            swift_installed: false,
            swift_ready: false,
            effective_provider: swiftSelected ? "deepgram" : prev.effective_provider,
          }
        : prev,
    );
    if (swiftSelected && prefs) {
      onPrefsUpdated({ ...prefs, stt_provider: "deepgram" });
    }

    try {
      await invoke("swift_stt_delete_model");
      if (swiftSelected) {
        const updated = await patchPreferences({ stt_provider: "deepgram" });
        if (updated) {
          onPrefsUpdated(updated);
        } else if (prefsSnapshot) {
          onPrefsUpdated(prefsSnapshot);
        }
      }
      showSuccess("Swift model deleted");
      await refresh();
    } catch (e) {
      if (modelSnapshot) setModel(modelSnapshot);
      if (runtimeSnapshot) setRuntime(runtimeSnapshot);
      if (prefsSnapshot) onPrefsUpdated(prefsSnapshot);
      setError(e instanceof Error ? e.message : String(e));
      await refresh();
    } finally {
      setDeletingSwift(false);
    }
  };

  const swiftDownloadPct =
    swiftDownload && swiftDownload.total > 0
      ? Math.min(100, Math.round((swiftDownload.received / swiftDownload.total) * 100))
      : null;
  const whisperDownloadPct =
    whisperDownload && whisperDownload.total > 0
      ? Math.min(100, Math.round((whisperDownload.received / whisperDownload.total) * 100))
      : null;

  return (
    <div className="panel overflow-hidden mb-7">
      <div className="px-5 py-4 border-b" style={{ borderColor: "hsl(var(--surface-3))" }}>
        <p className="text-[13px] font-medium text-foreground">Speech recognition</p>
        <p className="text-[12px] text-muted-foreground mt-0.5">
          On-device whisper.cpp (default, no Python) or cloud Deepgram for Caps Lock dictation.
        </p>
      </div>

      <div className="px-5 py-4 flex flex-col gap-3">
        <div className="grid grid-cols-1 sm:grid-cols-2 gap-2">
          <button
            type="button"
            disabled={busy || deletingSwift || !!swiftDownload}
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
                ? "Deepgram · API key configured"
                : "Deepgram · add API key below"}
            </p>
          </button>

          <button
            type="button"
            disabled={busy || deletingSwift || !!swiftDownload}
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
              On-device whisper.cpp · no Python, no cloud
            </p>
          </button>
        </div>

        {provider === "whisper_local" && (
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
                        : "Whisper-Hindi2Hinglish (GGML) · 141 MB"}
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
        )}

        {isMac && swiftSelected && (
          <div
            className="rounded-xl px-4 py-3"
            style={{ background: "hsl(var(--surface-2))" }}
          >
            <div className="flex items-center justify-between gap-3">
              <div className="min-w-0">
                <p className="text-[12px] font-medium text-foreground">Oriserve Swift</p>
                <p className="text-[11px] text-muted-foreground">
                  {deletingSwift
                    ? "Deleting model files…"
                    : swiftInstalled
                      ? `Installed · ${formatSize(model?.size_bytes ?? 0)}`
                      : "Whisper-Hindi2Hinglish-Swift · ~290 MB"}
                </p>
              </div>
              <div className="flex items-center gap-2 shrink-0">
                {deletingSwift ? (
                  <span className="flex items-center gap-1.5 text-[11px] text-muted-foreground">
                    <Loader2 size={12} className="animate-spin" />
                    Deleting…
                  </span>
                ) : swiftInstalled ? (
                  confirmDeleteSwift ? (
                    <span className="flex items-center gap-1">
                      <button
                        type="button"
                        onClick={() => void deleteSwiftModel()}
                        className="text-[11px] px-2 py-1 rounded-lg font-semibold"
                        style={{ background: "hsl(354 70% 30%)", color: "hsl(354 90% 90%)" }}
                      >
                        Confirm delete
                      </button>
                      <button
                        type="button"
                        onClick={() => setConfirmDeleteSwift(false)}
                        title="Cancel"
                        className="flex h-7 w-7 items-center justify-center rounded-lg text-muted-foreground hover:text-foreground"
                      >
                        <X size={14} />
                      </button>
                    </span>
                  ) : (
                    <>
                      <span className="text-[10px] text-emerald-600 font-medium">Ready</span>
                      <button
                        type="button"
                        disabled={busy || !!swiftDownload}
                        onClick={() => {
                          setConfirmDeleteSwift(true);
                          setError(null);
                          setSuccessMsg(null);
                        }}
                        className="flex items-center gap-1 text-[11px] px-2 py-1 rounded-lg border text-muted-foreground hover:text-foreground"
                        style={{ borderColor: "hsl(var(--surface-3))" }}
                        title="Delete model"
                      >
                        <Trash2 size={12} />
                        Delete
                      </button>
                    </>
                  )
                ) : swiftDownload ? (
                  <button
                    type="button"
                    onClick={() => void cancelSwiftDownload()}
                    className="text-[11px] px-2.5 py-1.5 rounded-lg border"
                    style={{ borderColor: "hsl(var(--surface-3))" }}
                  >
                    Cancel
                  </button>
                ) : (
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() => void startSwiftDownload()}
                    className="flex items-center gap-1.5 text-[11px] px-2.5 py-1.5 rounded-lg font-medium"
                    style={{
                      background: "hsl(var(--primary))",
                      color: "hsl(var(--primary-foreground))",
                    }}
                  >
                    {busy ? <Loader2 size={12} className="animate-spin" /> : <Download size={12} />}
                    Download
                  </button>
                )}
              </div>
            </div>
            {swiftDownload && swiftDownloadPct !== null && (
              <div className="mt-3">
                <div className="h-1.5 rounded-full overflow-hidden" style={{ background: "hsl(var(--surface-4))" }}>
                  <div
                    className="h-full transition-all"
                    style={{ width: `${swiftDownloadPct}%`, background: "hsl(var(--primary))" }}
                  />
                </div>
                <p className="text-[10px] text-muted-foreground mt-1">
                  Downloading {swiftDownloadPct}% · {formatSize(swiftDownload.received)} / {formatSize(swiftDownload.total)}
                </p>
              </div>
            )}
            {swiftInstalled && runtime?.swift_ready && !deletingSwift && (
              <p className="text-[10px] text-emerald-600 mt-2">Local inference engine ready</p>
            )}
            {swiftInstalled && swiftSelected && !runtime?.swift_ready && !deletingSwift && (
              <p className="text-[10px] mt-2" style={{ color: "hsl(38 92% 60%)" }}>
                Model installed, but the local engine isn’t running yet. It starts on your first
                dictation — if speech recognition fails, install the Swift sidecar’s Python
                requirements (see swift-stt-sidecar README).
              </p>
            )}
            {swiftInstalled && !swiftSelected && !deletingSwift && (
              <p className="text-[10px] text-muted-foreground mt-2">
                Model installed — select Local Swift above to use it.
              </p>
            )}
          </div>
        )}

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
