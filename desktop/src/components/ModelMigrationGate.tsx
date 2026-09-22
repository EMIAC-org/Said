import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { ArrowRight, Check, Cloud, Cpu, Loader2 } from "lucide-react";
import {
  chooseInstalledLocalModel,
  getLocalModelInventory,
  getSttSetupPolicy,
  invoke,
  removeUnusedLocalDictationModels,
  type LocalModelInfo,
  type LocalModelInventory,
  type SttSetupPolicy,
} from "@/lib/invoke";
import { friendlyError } from "@/lib/friendlyError";
import { ErrorNotice } from "./ErrorNotice";
import type { Platform } from "@/lib/hotkeys";

interface DownloadProgress {
  name: string;
  received: number;
  total: number;
  status: "downloading" | "done" | "cancelled" | "error" | string;
  error: string | null;
}

/** The dictation model download is a single Rust command reporting on a single
 *  event. It stayed a lookup table while three models existed; there is one now. */
const DOWNLOAD_COMMAND = "download_dictation_model";
const DOWNLOAD_EVENT = "meeting-model-download";
/** Progress events carry the destination filename, which is unchanged — the new
 *  model installs over the old one at the same path. */
const DOWNLOAD_EVENT_NAME = "ggml-oriserve-hinglish-fp16.bin";

function formatSize(bytes: number): string {
  if (bytes >= 1_000_000_000) return `${(bytes / 1_000_000_000).toFixed(1)} GB`;
  return `${Math.max(1, Math.round(bytes / 1_000_000))} MB`;
}

/**
 * Required speech-setup update, run once per migration version.
 *
 * AirNote ships one local model now, so there is nothing for the user to choose
 * and nothing to keep as a rollback — an older model cannot be selected any
 * more. The gate therefore starts the download itself and reports progress
 * rather than asking permission. Rust replaces the previous file in place and
 * verifies the checksum before the model counts as installed.
 */
export function ModelMigrationGate({ onDone, platform: _platform }: { onDone: () => void; platform: Platform }) {
  const [policy, setPolicy] = useState<SttSetupPolicy | null>(null);
  const [inventory, setInventory] = useState<LocalModelInventory | null>(null);
  const [download, setDownload] = useState<DownloadProgress | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const mounted = useRef(true);
  /** Guards the auto-start so a re-render, or a refresh triggered by a progress
   *  event, cannot launch a second download of the same file. */
  const started = useRef(false);

  const refresh = useCallback(async () => {
    try {
      const [nextPolicy, nextInventory] = await Promise.all([
        getSttSetupPolicy(),
        getLocalModelInventory(),
      ]);
      if (!mounted.current) return null;
      setPolicy(nextPolicy);
      setInventory(nextInventory);
      return nextInventory;
    } catch (cause) {
      if (mounted.current) setError(friendlyError(cause));
      return null;
    }
  }, []);

  useEffect(() => {
    mounted.current = true;
    void refresh();
    return () => { mounted.current = false; };
  }, [refresh]);

  useEffect(() => {
    if (inventory?.setup_kind !== "local_required") return;
    const unlisten = listen<DownloadProgress>(DOWNLOAD_EVENT, (event) => {
      const progress = event.payload;
      if (progress.name !== DOWNLOAD_EVENT_NAME) return;
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
  }, [inventory?.setup_kind, refresh]);

  /** Download the current model, select it, and clear out anything an older
   *  release left behind. Retired files are removed without asking: nothing can
   *  load them any more, so keeping them is lost disk, not a rollback. */
  const install = useCallback(async () => {
    const recommended = inventory?.recommended_model;
    if (!recommended) return;
    setBusy(true);
    setError("");
    try {
      await invoke(DOWNLOAD_COMMAND, undefined);
      const next = await chooseInstalledLocalModel(recommended);
      if (next.reclaimable_bytes > 0) {
        await removeUnusedLocalDictationModels();
      }
      if (!mounted.current) return;
      await refresh();
      onDone();
    } catch (cause) {
      const message = friendlyError(cause);
      // A cancel is a user action, not a failure worth shouting about. Reopen
      // the door so a retry can start a fresh attempt.
      started.current = false;
      if (message.toLowerCase() !== "cancelled") setError(message);
    } finally {
      if (mounted.current) setBusy(false);
    }
  }, [inventory?.recommended_model, onDone, refresh]);

  // Start as soon as we know what this machine needs. An already-current model
  // means there is nothing to do and the gate closes on its own.
  useEffect(() => {
    if (!inventory || inventory.setup_kind !== "local_required") return;
    if (started.current || busy) return;
    const recommended = inventory.models.find((model) => model.key === inventory.recommended_model);
    if (recommended?.installed && recommended.active_for_dictation && inventory.reclaimable_bytes === 0) {
      started.current = true;
      onDone();
      return;
    }
    started.current = true;
    void install();
  }, [inventory, busy, install, onDone]);

  if (!policy || !inventory) {
    return (
      <div className="mig-overlay" role="dialog" aria-modal="true" aria-labelledby="model-migration-title">
        <div className="mig-card">
          {error ? (
            <>
              <div className="mig-badge"><Cpu size={12} /> Updated speech setup</div>
              <h2 id="model-migration-title" className="mig-title">Couldn’t check this device.</h2>
              <p className="mig-desc">AirNote must inspect the installed speech models before continuing.</p>
              <ErrorNotice error={error} onRetry={() => void refresh()} className="mt-3" />
              <div className="mig-actions"><button onClick={() => void refresh()} className="btn-primary btn-lg w-full">Try again</button></div>
            </>
          ) : <Loader2 className="animate-spin" size={18} aria-label="Checking installed speech models" />}
        </div>
      </div>
    );
  }

  if (policy.setup_kind === "cloud_locked") {
    return (
      <div className="mig-overlay" role="dialog" aria-modal="true" aria-labelledby="model-migration-title">
        <div className="mig-card">
          <div className="mig-badge"><Cloud size={12} /> Updated speech setup</div>
          <h2 id="model-migration-title" className="mig-title">Cloud Whisper is enabled.</h2>
          <p className="mig-desc">
            {policy.cpu_family === "intel" ? "This Intel Mac" : "Windows"} now uses DeepInfra cloud speech recognition for dictation. No local dictation download is needed.
          </p>
          <div className="mig-model">
            <div className="mig-model-row">
              <span className="mig-model-left"><span className="mig-model-ico"><Cloud size={13} /></span><span className="mig-model-name">Whisper Large V3 Turbo · DeepInfra</span></span>
              <span className="mig-ready"><Check size={12} /> Ready</span>
            </div>
          </div>
          <div className="mig-actions"><button onClick={onDone} className="btn-primary btn-lg w-full">Continue <ArrowRight size={14} /></button></div>
        </div>
      </div>
    );
  }

  const recommended = inventory.models.find((model) => model.key === inventory.recommended_model) as LocalModelInfo | undefined;
  const pct = download && download.total > 0 ? Math.min(100, Math.round((download.received / download.total) * 100)) : null;
  const reclaiming = recommended?.installed && inventory.reclaimable_bytes > 0;

  const title = recommended?.installed
    ? "Finishing up."
    : `Updating your speech model.`;
  const description = recommended?.installed
    ? `Freeing ${formatSize(inventory.reclaimable_bytes)} from speech models AirNote no longer uses.`
    : `AirNote is installing ${recommended?.name ?? "the current model"} for this Mac. This replaces the older model and takes a moment.`;

  return (
    <div className="mig-overlay" role="dialog" aria-modal="true" aria-labelledby="model-migration-title">
      <div className="mig-card">
        <div className="mig-badge"><Cpu size={12} /> Updated local speech setup</div>
        <h2 id="model-migration-title" className="mig-title">{title}</h2>
        <p className="mig-desc">{description}</p>
        <div className="mig-model" aria-live="polite">
          <div className="mig-model-row">
            <span className="mig-model-left"><span className="mig-model-ico"><Cpu size={13} /></span><span className="mig-model-name">{recommended?.name} · {recommended?.size_hint}</span></span>
            {recommended?.installed && !reclaiming
              ? <span className="mig-ready"><Check size={12} /> Installed</span>
              : <span className="mig-ready"><Loader2 size={12} className="animate-spin" /> {pct !== null ? `${pct}%` : "Working…"}</span>}
          </div>
          {pct !== null && <div className="mig-bar"><div style={{ width: `${Math.max(4, pct)}%` }} /></div>}
          <ErrorNotice error={error} onRetry={() => void install()} className="mt-2" />
        </div>

        {/* No actions while this runs itself. A failure is the only thing the
            user can act on, and ErrorNotice above already offers the retry. */}
      </div>
    </div>
  );
}
