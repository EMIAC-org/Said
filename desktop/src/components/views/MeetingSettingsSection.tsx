import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Check, Download, Loader2, Trash2, X } from "lucide-react";

// Keys mirror the engine's env-var names; the settings store reads them back.
const LANG_KEYS = [
  "AIRNOTE_MEETING_MIC_WHISPER_LANGUAGE",
  "AIRNOTE_MEETING_SYSTEM_WHISPER_LANGUAGE",
  "AIRNOTE_MEETING_WHISPER_LANGUAGE",
] as const;
const STALE_KEYS = [
  "AIRNOTE_MEETING_LIVE_WHISPER_CPP_MODEL",
  "AIRNOTE_MEETING_FINAL_DIARIZATION_MODE",
  "AIRNOTE_MEETING_FINAL_DIARIZATION_ENABLED",
  "AIRNOTE_MEETING_FINAL_DIARIZATION_PROVIDER",
  "AIRNOTE_MEETING_FINAL_DIARIZATION_SCRIPT",
  "AIRNOTE_MEETING_FINAL_DIARIZATION_COMMAND",
  "AIRNOTE_MEETING_FINAL_DIARIZATION_PYTHON",
  "AIRNOTE_MEETING_LIGHT_DIARIZATION_MAX_SPEAKERS",
  "AIRNOTE_MEETING_LIGHT_DIARIZATION_MAX_AUDIO_SECS",
] as const;
const FINAL_MODEL_KEY = "AIRNOTE_WHISPER_CPP_MODEL";

interface InstalledModel {
  name: string;
  path: string;
  size_bytes: number;
  active: boolean;
  incomplete: boolean;
}
interface CatalogModel {
  name: string;
  size_bytes: number;
  installed: boolean;
}
interface DownloadProgress {
  name: string;
  received: number;
  total: number;
  status: "downloading" | "done" | "cancelled" | "error";
  error: string | null;
}
interface RemovedModel {
  name: string;
  size_bytes: number;
}
interface ModelCleanupResult {
  removed: RemovedModel[];
  freed_bytes: number;
}
function formatSize(bytes: number): string {
  if (bytes >= 1e9) return `${(bytes / 1e9).toFixed(1)} GB`;
  if (bytes >= 1e6) return `${Math.round(bytes / 1e6)} MB`;
  if (bytes > 0) return `${Math.round(bytes / 1e3)} KB`;
  return "—";
}

function prettyModelName(name: string): string {
  if (name === "ggml-large-v3-turbo-q5_0.bin") return "Turbo Q5";
  return name.replace(/^ggml-/, "").replace(/\.bin$/, "");
}

function downloadModelHint(name: string): string {
  const n = name.toLowerCase();
  if (n === "ggml-large-v3-turbo-q5_0.bin") return "Best balance for normal laptops";
  return "Downloadable model";
}

export function MeetingSettingsSection() {
  const [installed, setInstalled] = useState<InstalledModel[]>([]);
  const [catalog, setCatalog] = useState<CatalogModel[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  // name → progress while downloading; cleared on done/cancel/error.
  const [downloads, setDownloads] = useState<Record<string, DownloadProgress>>({});
  const [busyKey, setBusyKey] = useState<string | null>(null);
  const [cleanupBusy, setCleanupBusy] = useState(false);
  const [cleanupMessage, setCleanupMessage] = useState<string | null>(null);
  const mounted = useRef(true);

  const refresh = useCallback(async () => {
    try {
      // Guarantee one model is active when any is installed (auto-selects the
      // low-memory recommendation, clears when none remain) before reading the list.
      await invoke<string | null>("meeting_ensure_active_model").catch(() => null);
      const [fetchedSettings, models, cat] = await Promise.all([
        invoke<Record<string, string>>("meeting_settings_get"),
        invoke<InstalledModel[]>("meeting_list_whisper_models"),
        invoke<CatalogModel[]>("meeting_whisper_model_catalog"),
      ]);
      if (!mounted.current) return;
      // Meeting language is fixed in the engine. Clear any stale
      // per-track language overrides a previous build's picker may have stored,
      // so the engine default wins.
      [...LANG_KEYS, ...STALE_KEYS].forEach((key) => {
        if (fetchedSettings[key]) void invoke("meeting_settings_set", { key, value: null }).catch(() => {});
      });
      setInstalled(models);
      setCatalog(cat);
      setError(null);
    } catch (e) {
      if (mounted.current) setError(e instanceof Error ? e.message : String(e));
    } finally {
      if (mounted.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    mounted.current = true;
    void refresh();
    return () => {
      mounted.current = false;
    };
  }, [refresh]);

  // Model download progress.
  useEffect(() => {
    const unlistenP = listen<DownloadProgress>("meeting-model-download", (event) => {
      const p = event.payload;
      setDownloads((prev) => {
        const next = { ...prev };
        if (p.status === "downloading") next[p.name] = p;
        else delete next[p.name];
        return next;
      });
      if (p.status === "done") void refresh();
      if (p.status === "error" && p.error) setError(`${prettyModelName(p.name)}: ${p.error}`);
    });
    return () => {
      void unlistenP.then((fn) => fn());
    };
  }, [refresh]);

  const selectFinalModel = useCallback(
    async (path: string) => {
      setBusyKey(path);
      try {
        await invoke("meeting_settings_set", { key: FINAL_MODEL_KEY, value: path });
        await refresh();
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
      } finally {
        setBusyKey(null);
      }
    },
    [refresh],
  );

  // Inline confirm (window.confirm is a no-op in Tauri webviews).
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);
  const deleteModel = useCallback(
    async (name: string) => {
      setConfirmDelete(null);
      setBusyKey(name);
      try {
        await invoke("meeting_delete_whisper_model", { name });
        await refresh();
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
      } finally {
        setBusyKey(null);
      }
    },
    [refresh],
  );

  const clearDownload = useCallback((name: string) => {
    setDownloads((prev) => {
      const next = { ...prev };
      delete next[name];
      return next;
    });
  }, []);

  const download = useCallback(
    (name: string) => {
      setDownloads((prev) => ({
        ...prev,
        [name]: { name, received: 0, total: 0, status: "downloading", error: null },
      }));
      invoke("meeting_download_whisper_model", { name })
        .then(() => {
          // Completed (or was already installed): move it into the downloaded
          // list. Don't rely solely on the final progress event arriving.
          clearDownload(name);
          void refresh();
        })
        .catch((e) => {
          const msg = e instanceof Error ? e.message : String(e);
          if (msg !== "cancelled") setError(`${prettyModelName(name)}: ${msg}`);
          clearDownload(name);
        });
    },
    [clearDownload, refresh],
  );

  const cancelDownload = useCallback((name: string) => {
    void invoke("meeting_cancel_model_download", { name }).catch(() => {});
  }, []);

  const cleanupLegacyModels = useCallback(async () => {
    setCleanupBusy(true);
    setCleanupMessage(null);
    try {
      const result = await invoke<ModelCleanupResult>("meeting_cleanup_legacy_whisper_models");
      await refresh();
      if (result.removed.length === 0) {
        setCleanupMessage("No old models found.");
      } else {
        setCleanupMessage(`Removed ${result.removed.length} old model${result.removed.length === 1 ? "" : "s"} and freed ${formatSize(result.freed_bytes)}.`);
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setCleanupBusy(false);
    }
  }, [refresh]);

  const notInstalled = catalog
    .filter((c) => !c.installed && !installed.some((m) => m.name === c.name));

  return (
    <div className="space-y-7">
      {error ? (
        <div
          className="flex items-start gap-2 rounded-lg px-3 py-2 text-[12px]"
          style={{ background: "hsl(354 60% 14%)", color: "hsl(354 85% 80%)" }}
        >
          <X size={14} className="mt-0.5 flex-shrink-0" />
          <span className="min-w-0 flex-1">{error}</span>
          <button type="button" onClick={() => setError(null)} className="opacity-70 hover:opacity-100">
            Dismiss
          </button>
        </div>
      ) : null}

      {/* Transcript model */}
      <section>
        <h3 className="text-[14px] font-bold text-foreground">Transcript model</h3>
        <p className="mt-1 text-[12px] text-muted-foreground">
          AirNote uses one model for live chunks and the full transcript after the meeting: Turbo
          Q5. It keeps memory lower on normal laptops.
        </p>

        {loading ? (
          <div className="mt-3 flex items-center gap-2 text-[12px] text-muted-foreground">
            <Loader2 size={14} className="animate-spin" /> Loading models…
          </div>
        ) : (
          <div className="mt-3 space-y-2">
            {installed.length === 0 ? (
              <p className="text-[12px] text-muted-foreground">Turbo Q5 is not installed yet — download it below.</p>
            ) : (
              installed.map((m) => (
                <div
                  key={m.path}
                  className="flex items-center gap-3 rounded-lg px-3 py-2"
                  style={{
                    background: m.active ? "hsl(var(--primary) / 0.10)" : "hsl(var(--surface-3))",
                    border: `1px solid ${m.active ? "hsl(var(--primary) / 0.30)" : "hsl(var(--surface-4))"}`,
                  }}
                >
                  <button
                    type="button"
                    onClick={() => !m.active && !m.incomplete && void selectFinalModel(m.path)}
                    disabled={busyKey === m.path || m.incomplete}
                    className="flex min-w-0 flex-1 items-center gap-2.5 text-left disabled:cursor-default"
                    title={
                      m.incomplete
                        ? "Incomplete download — delete and re-download"
                        : m.active
                          ? "Active transcript model"
                          : "Use this transcript model"
                    }
                  >
                    <span
                      className="flex h-4 w-4 flex-shrink-0 items-center justify-center rounded-full"
                      style={{
                        background: m.active ? "hsl(var(--primary))" : "transparent",
                        border: m.active ? "none" : "1.5px solid hsl(var(--muted-foreground) / 0.5)",
                        opacity: m.incomplete ? 0.4 : 1,
                      }}
                    >
                      {m.active ? <Check size={11} style={{ color: "hsl(var(--primary-foreground))" }} /> : null}
                    </span>
                    <span className="min-w-0">
                      <span className="block truncate text-[13px] font-semibold text-foreground">
                        {prettyModelName(m.name)}
                      </span>
                      <span className="text-[11px]" style={{ color: m.incomplete ? "hsl(38 90% 66%)" : "hsl(var(--muted-foreground))" }}>
                        {m.incomplete ? "Incomplete — re-download" : `Ready · ${formatSize(m.size_bytes)}`}
                      </span>
                    </span>
                  </button>
                  {busyKey === m.path ? (
                    <span className="flex h-7 w-7 flex-shrink-0 items-center justify-center text-muted-foreground">
                      <Loader2 size={13} className="animate-spin" />
                    </span>
                  ) : confirmDelete === m.name ? (
                    <span className="flex flex-shrink-0 items-center gap-1">
                      <button
                        type="button"
                        onClick={() => void deleteModel(m.name)}
                        title="Confirm delete"
                        className="h-7 rounded-lg px-2 text-[11px] font-bold"
                        style={{ background: "hsl(354 70% 30%)", color: "hsl(354 90% 90%)" }}
                      >
                        Delete
                      </button>
                      <button
                        type="button"
                        onClick={() => setConfirmDelete(null)}
                        title="Cancel"
                        className="flex h-7 w-7 items-center justify-center rounded-lg text-muted-foreground hover:text-foreground"
                      >
                        <X size={14} />
                      </button>
                    </span>
                  ) : (
                    <button
                      type="button"
                      onClick={() => setConfirmDelete(m.name)}
                      title="Delete model"
                      className="flex h-7 w-7 flex-shrink-0 items-center justify-center rounded-lg text-muted-foreground transition-colors hover:text-[hsl(354_85%_70%)]"
                    >
                      <Trash2 size={13} />
                    </button>
                  )}
                </div>
              ))
            )}
          </div>
        )}
      </section>

      {/* Downloads */}
      <section>
        <h3 className="text-[14px] font-bold text-foreground">Model downloads</h3>
        <p className="mt-1 text-[12px] text-muted-foreground">
          Keep only Turbo Q5 on disk. Use cleanup to remove older model files from previous builds.
        </p>
        {loading ? null : (
          <div className="mt-3 space-y-2">
            {notInstalled.length > 0 ? (
              <p className="text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">Available</p>
            ) : (
              <p className="text-[12px] text-muted-foreground">Turbo Q5 is installed.</p>
            )}
            {notInstalled.map((c) => {
              const dl = downloads[c.name];
              const pct = dl && dl.total > 0 ? Math.round((dl.received / dl.total) * 100) : null;
              return (
                <div
                  key={c.name}
                  className="rounded-lg px-3 py-2"
                  style={{ background: "hsl(var(--surface-3))", border: "1px solid hsl(var(--surface-4))" }}
                >
                  <div className="flex items-center gap-3">
                    <span className="min-w-0 flex-1">
                      <span className="block truncate text-[13px] font-semibold text-foreground">
                        {prettyModelName(c.name)}
                      </span>
                      <span className="text-[11px] text-muted-foreground">
                        {downloadModelHint(c.name)} · ~{formatSize(c.size_bytes)}
                      </span>
                    </span>
                    {dl ? (
                      <button
                        type="button"
                        onClick={() => cancelDownload(c.name)}
                        className="flex h-7 items-center gap-1.5 rounded-lg px-2.5 text-[12px] font-semibold text-muted-foreground hover:text-foreground"
                      >
                        <Loader2 size={13} className="animate-spin" />
                        {pct !== null ? `${pct}%` : "…"} · Cancel
                      </button>
                    ) : (
                      <button
                        type="button"
                        onClick={() => download(c.name)}
                        className="flex h-7 items-center gap-1.5 rounded-lg px-2.5 text-[12px] font-bold"
                        style={{ background: "hsl(var(--primary))", color: "hsl(var(--primary-foreground))" }}
                      >
                        <Download size={13} /> Download
                      </button>
                    )}
                  </div>
                  {dl ? (
                    <div className="mt-2 h-1 w-full overflow-hidden rounded-full" style={{ background: "hsl(var(--surface-4))" }}>
                      <div
                        className="h-full rounded-full transition-all"
                        style={{ width: `${pct ?? 5}%`, background: "hsl(var(--primary))" }}
                      />
                    </div>
                  ) : null}
                </div>
              );
            })}
            <div
              className="rounded-lg px-3 py-2"
              style={{ background: "hsl(var(--surface-3))", border: "1px solid hsl(var(--surface-4))" }}
            >
              <div className="flex items-center gap-3">
                <span className="min-w-0 flex-1">
                  <span className="block text-[13px] font-semibold text-foreground">Clean up old models</span>
                  <span className="text-[11px] text-muted-foreground">
                    Removes older Whisper downloads AirNote no longer uses. Turbo Q5 stays.
                  </span>
                </span>
                <button
                  type="button"
                  onClick={() => void cleanupLegacyModels()}
                  disabled={cleanupBusy}
                  className="flex h-7 items-center gap-1.5 rounded-lg px-2.5 text-[12px] font-semibold text-muted-foreground hover:text-foreground disabled:cursor-not-allowed disabled:opacity-60"
                >
                  {cleanupBusy ? <Loader2 size={13} className="animate-spin" /> : <Trash2 size={13} />}
                  Clean up
                </button>
              </div>
              {cleanupMessage ? <p className="mt-2 text-[11px] text-muted-foreground">{cleanupMessage}</p> : null}
            </div>
          </div>
        )}
      </section>
    </div>
  );
}
