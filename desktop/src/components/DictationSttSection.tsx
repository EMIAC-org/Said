import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Check, Cloud, Cpu, Download, Loader2, Trash2, X } from "lucide-react";
import type { Preferences, SttRuntimeInfo } from "../types";
import { getSttRuntime, patchPreferences } from "../lib/invoke";

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
  const [download, setDownload] = useState<SwiftDownloadProgress | null>(null);
  const [busy, setBusy] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [successMsg, setSuccessMsg] = useState<string | null>(null);
  const mounted = useRef(true);
  const successTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const provider = prefs?.stt_provider ?? "deepgram";
  const swiftSelected = provider === "swift_local";
  const modelInstalled = deleting ? false : (model?.installed ?? false);

  const showSuccess = useCallback((msg: string) => {
    if (successTimer.current) clearTimeout(successTimer.current);
    setSuccessMsg(msg);
    successTimer.current = setTimeout(() => {
      if (mounted.current) setSuccessMsg(null);
    }, 3200);
  }, []);

  const refresh = useCallback(async () => {
    try {
      const [rt, status] = await Promise.all([
        getSttRuntime(),
        invoke<SwiftModelStatus>("swift_stt_model_status").catch(() => null),
      ]);
      if (!mounted.current) return;
      setRuntime(rt);
      if (status) setModel(status);
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
    const unlistenP = listen<SwiftDownloadProgress>("swift-model-download", (event) => {
      const p = event.payload;
      if (p.status === "downloading") {
        setDownload(p);
        setError(null);
        setSuccessMsg(null);
      } else {
        setDownload(null);
      }
      if (p.status === "done") {
        showSuccess("Model downloaded");
        void refresh();
      }
      if (p.status === "error" && p.error) setError(p.error);
    });
    return () => {
      void unlistenP.then((fn) => fn());
    };
  }, [refresh, showSuccess]);

  const selectProvider = async (next: "deepgram" | "swift_local") => {
    if (next === provider) return;
    if (next === "swift_local" && !modelInstalled && !deleting) {
      setError("Download the Swift model below, then select Local.");
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

  const startDownload = async () => {
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

  const cancelDownload = async () => {
    await invoke("swift_stt_cancel_download").catch(() => {});
    setDownload(null);
  };

  const deleteModel = async () => {
    setConfirmDelete(false);
    setDeleting(true);
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
        ? { ...prev, swift_installed: false, swift_ready: false, effective_provider: swiftSelected ? "deepgram" : prev.effective_provider }
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
      setDeleting(false);
    }
  };

  const downloadPct =
    download && download.total > 0
      ? Math.min(100, Math.round((download.received / download.total) * 100))
      : null;

  return (
    <div className="panel overflow-hidden mb-7">
      <div className="px-5 py-4 border-b" style={{ borderColor: "hsl(var(--surface-3))" }}>
        <p className="text-[13px] font-medium text-foreground">Speech recognition</p>
        <p className="text-[12px] text-muted-foreground mt-0.5">
          Choose cloud Deepgram or local Swift Hinglish STT for Caps Lock dictation.
        </p>
      </div>

      <div className="px-5 py-4 flex flex-col gap-3">
        <div className="flex gap-2">
          <button
            type="button"
            disabled={busy || deleting || !!download}
            onClick={() => void selectProvider("deepgram")}
            className="flex-1 rounded-xl px-3 py-2.5 text-left border transition-colors"
            style={{
              borderColor:
                provider === "deepgram" ? "hsl(var(--primary))" : "hsl(var(--surface-3))",
              background:
                provider === "deepgram" ? "hsl(var(--surface-3))" : "hsl(var(--surface-2))",
            }}
          >
            <div className="flex items-center gap-2 text-[13px] font-medium text-foreground">
              <Cloud size={14} />
              Cloud — Deepgram
              {provider === "deepgram" && <Check size={14} className="ml-auto text-primary" />}
            </div>
            <p className="text-[11px] text-muted-foreground mt-1">
              {runtime?.deepgram_configured
                ? "API key configured"
                : "Add a Deepgram key in API Keys"}
            </p>
          </button>

          <button
            type="button"
            disabled={busy || deleting || !isMac || !!download}
            onClick={() => void selectProvider("swift_local")}
            className="flex-1 rounded-xl px-3 py-2.5 text-left border transition-colors disabled:opacity-50"
            style={{
              borderColor:
                provider === "swift_local" ? "hsl(var(--primary))" : "hsl(var(--surface-3))",
              background:
                provider === "swift_local" ? "hsl(var(--surface-3))" : "hsl(var(--surface-2))",
            }}
          >
            <div className="flex items-center gap-2 text-[13px] font-medium text-foreground">
              <Cpu size={14} />
              Local — Swift Hinglish
              {!isMac && (
                <span className="text-[10px] px-1.5 py-0.5 rounded bg-surface-4 text-muted-foreground ml-1">
                  macOS only
                </span>
              )}
              {provider === "swift_local" && <Check size={14} className="ml-auto text-primary" />}
            </div>
            <p className="text-[11px] text-muted-foreground mt-1">
              {modelInstalled ? "Model ready on this Mac" : deleting ? "Removing model…" : "Requires ~290 MB download"}
            </p>
          </button>
        </div>

        {isMac && (
          <div
            className="rounded-xl px-4 py-3"
            style={{ background: "hsl(var(--surface-2))" }}
          >
            <div className="flex items-center justify-between gap-3">
              <div className="min-w-0">
                <p className="text-[12px] font-medium text-foreground">Oriserve Swift</p>
                <p className="text-[11px] text-muted-foreground">
                  {deleting
                    ? "Deleting model files…"
                    : modelInstalled
                      ? `Installed · ${formatSize(model?.size_bytes ?? 0)}`
                      : "Whisper-Hindi2Hinglish-Swift · ~290 MB"}
                </p>
              </div>
              <div className="flex items-center gap-2 shrink-0">
                {deleting ? (
                  <span className="flex items-center gap-1.5 text-[11px] text-muted-foreground">
                    <Loader2 size={12} className="animate-spin" />
                    Deleting…
                  </span>
                ) : modelInstalled ? (
                  confirmDelete ? (
                    <span className="flex items-center gap-1">
                      <button
                        type="button"
                        onClick={() => void deleteModel()}
                        className="text-[11px] px-2 py-1 rounded-lg font-semibold"
                        style={{ background: "hsl(354 70% 30%)", color: "hsl(354 90% 90%)" }}
                      >
                        Confirm delete
                      </button>
                      <button
                        type="button"
                        onClick={() => setConfirmDelete(false)}
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
                        disabled={busy || !!download}
                        onClick={() => {
                          setConfirmDelete(true);
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
                ) : download ? (
                  <button
                    type="button"
                    onClick={() => void cancelDownload()}
                    className="text-[11px] px-2.5 py-1.5 rounded-lg border"
                    style={{ borderColor: "hsl(var(--surface-3))" }}
                  >
                    Cancel
                  </button>
                ) : (
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() => void startDownload()}
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
            {download && downloadPct !== null && (
              <div className="mt-3">
                <div className="h-1.5 rounded-full overflow-hidden" style={{ background: "hsl(var(--surface-4))" }}>
                  <div
                    className="h-full transition-all"
                    style={{ width: `${downloadPct}%`, background: "hsl(var(--primary))" }}
                  />
                </div>
                <p className="text-[10px] text-muted-foreground mt-1">
                  Downloading {downloadPct}% · {formatSize(download.received)} / {formatSize(download.total)}
                </p>
              </div>
            )}
            {modelInstalled && runtime?.swift_ready && !deleting && (
              <p className="text-[10px] text-emerald-600 mt-2">Local inference engine ready</p>
            )}
            {modelInstalled && !swiftSelected && !deleting && (
              <p className="text-[10px] text-muted-foreground mt-2">
                Model installed — select Local above to use it.
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
