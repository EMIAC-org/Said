import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import { ArrowRight, Check, Cloud, Cpu, Loader2 } from "lucide-react";
import {
  chooseInstalledLocalModel,
  getDesktopPrefs,
  getLocalModelInventory,
  getSttSetupPolicy,
  invoke,
  removeUnusedLocalDictationModels,
  setDesktopPrefs,
  type LocalModelInfo,
  type LocalModelInventory,
  type SttSetupPolicy,
} from "@/lib/invoke";
import { friendlyError } from "@/lib/friendlyError";
import { ErrorNotice } from "./ErrorNotice";
import type { Platform } from "@/lib/hotkeys";
import { NEW_MODEL_FILE, NEW_MODEL_NAME, NEW_MODEL_SIZE_HINT } from "@/lib/onDeviceModel";

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
/** The retired model's inventory key. Its file still loads, so a machine that
 *  has it can keep dictating while the new one downloads. */
const LEGACY_MODEL_KEY = "oriserve";
const CLOUD_ROUTE = "cloud-deepinfra-whisper-v3-turbo";

/**
 * Required speech-setup update, run once per migration version.
 *
 * AirNote ships one local model now, so there is nothing for the user to choose.
 * The gate starts the download itself and reports progress. Rust downloads to a
 * staging file and only swaps it in after the checksum passes, so the previous
 * model keeps working until the new one is proven.
 *
 * The gate never traps anyone. A user who still has the older model can carry
 * on with it and let the download finish in the background. A user with no
 * model at all can cancel, or fall back to cloud speech. Either way the
 * migration is only stamped done after the new model is installed, so the next
 * launch tries again by itself.
 */
export function ModelMigrationGate({
  onDone,
  onDismiss,
  platform: _platform,
}: {
  /** The new model is installed: never show this again. */
  onDone: () => void;
  /** Close for this session only; the next launch retries. */
  onDismiss: () => void;
  platform: Platform;
}) {
  const [policy, setPolicy] = useState<SttSetupPolicy | null>(null);
  const [inventory, setInventory] = useState<LocalModelInventory | null>(null);
  const [download, setDownload] = useState<DownloadProgress | null>(null);
  const [busy, setBusy] = useState(false);
  const [verifying, setVerifying] = useState(false);
  const [error, setError] = useState("");
  const [stopped, setStopped] = useState(false);
  const [switching, setSwitching] = useState(false);
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
      if (progress.name !== NEW_MODEL_FILE) return;
      if (progress.status === "downloading") {
        setDownload(progress);
        setError("");
      } else {
        setDownload(null);
      }
      // "done" means the bytes arrived; the checksum and swap still follow.
      setVerifying(progress.status === "done");
      if (progress.status === "error" && progress.error) setError(friendlyError(progress.error));
    });
    return () => { void unlisten.then((stop) => stop()); };
  }, [inventory?.setup_kind]);

  /** Download the current model, select it, and clear out anything an older
   *  release left behind. Retired files are removed without asking: nothing can
   *  load them any more, so keeping them is lost disk, not a rollback.
   *
   *  This keeps running after the gate is dismissed ("Continue in background"),
   *  which is why the success path does not bail out on unmount: the migration
   *  must still be stamped once the model is really there. */
  const install = useCallback(async () => {
    const recommended = inventory?.recommended_model;
    if (!recommended) return;
    setBusy(true);
    setStopped(false);
    setError("");
    try {
      await invoke(DOWNLOAD_COMMAND, undefined);
      const next = await chooseInstalledLocalModel(recommended);
      if (next.reclaimable_bytes > 0) {
        await removeUnusedLocalDictationModels();
      }
      onDone();
    } catch (cause) {
      if (!mounted.current) return;
      const message = friendlyError(cause);
      if (message.toLowerCase() === "cancelled") setStopped(true);
      else setError(message);
    } finally {
      if (mounted.current) {
        setBusy(false);
        setVerifying(false);
      }
    }
  }, [inventory?.recommended_model, onDone]);

  const cancel = useCallback(async () => {
    await invoke("meeting_cancel_model_download", { name: NEW_MODEL_FILE }).catch(() => {});
  }, []);

  /** Only offered when there is no model at all. Dictation works right away on
   *  cloud speech; the next launch installs the model and switches back. */
  const useCloudForNow = useCallback(async () => {
    setSwitching(true);
    setError("");
    try {
      const prefs = await getDesktopPrefs();
      await setDesktopPrefs({ ...prefs, dictation_stt: CLOUD_ROUTE });
      onDismiss();
    } catch (cause) {
      if (mounted.current) setError(friendlyError(cause));
    } finally {
      if (mounted.current) setSwitching(false);
    }
  }, [onDismiss]);

  // Start as soon as we know what this machine needs. An already-current model
  // means there is nothing to do and the gate closes on its own.
  useEffect(() => {
    if (!inventory || inventory.setup_kind !== "local_required") return;
    if (started.current || busy) return;
    const recommended = inventory.models.find((model) => model.key === inventory.recommended_model);
    started.current = true;
    if (recommended?.installed && recommended.active_for_dictation && inventory.reclaimable_bytes === 0) {
      onDone();
      return;
    }
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
  // The retired Oriserve file still loads, so dictation keeps working on it
  // while the new model downloads. That decides what the way out looks like.
  const hasWorkingModel = inventory.models.some((model) => model.key === LEGACY_MODEL_KEY && model.installed);
  const pct = download && download.total > 0 ? Math.min(100, Math.round((download.received / download.total) * 100)) : null;
  const failed = !busy && (Boolean(error) || stopped);
  const modelLabel = `${recommended?.name ?? NEW_MODEL_NAME} · ${recommended?.size_hint ?? NEW_MODEL_SIZE_HINT}`;

  let title: string;
  let description: string;
  if (failed) {
    title = stopped ? "Update paused." : "Couldn’t finish the update.";
    description = hasWorkingModel
      ? "Nothing changed. You’re still on your current speech model, and AirNote will try again the next time it opens."
      : "Dictation needs a speech model. Try again, or use cloud speech for now and AirNote will install the model the next time it opens.";
  } else if (recommended?.installed) {
    title = "Finishing up.";
    description = "Removing speech models AirNote no longer uses.";
  } else {
    title = "Updating your speech model.";
    description = hasWorkingModel
      ? `AirNote is installing ${recommended?.name ?? NEW_MODEL_NAME}, a sharper Hinglish model. Your current model keeps working until the new one is ready.`
      : `AirNote is installing ${recommended?.name ?? NEW_MODEL_NAME} for on-device dictation. It’s a one-time download.`;
  }

  let status: ReactNode;
  if (failed) status = null;
  else if (recommended?.installed && !busy) status = <span className="mig-ready"><Check size={12} /> Installed</span>;
  else if (verifying) status = <span className="mig-ready"><Loader2 size={12} className="animate-spin" /> Verifying…</span>;
  else status = <span className="mig-ready"><Loader2 size={12} className="animate-spin" /> {pct !== null ? `${pct}%` : "Starting…"}</span>;

  return (
    <div className="mig-overlay" role="dialog" aria-modal="true" aria-labelledby="model-migration-title">
      <div className="mig-card">
        <div className="mig-badge"><Cpu size={12} /> Updated local speech setup</div>
        <h2 id="model-migration-title" className="mig-title">{title}</h2>
        <p className="mig-desc">{description}</p>
        <div className="mig-model" aria-live="polite">
          <div className="mig-model-row">
            <span className="mig-model-left"><span className="mig-model-ico"><Cpu size={13} /></span><span className="mig-model-name">{modelLabel}</span></span>
            {status}
          </div>
          {!failed && pct !== null && <div className="mig-bar"><div style={{ width: `${Math.max(4, pct)}%` }} /></div>}
          {failed && error && <ErrorNotice error={error} className="mt-2" />}
        </div>

        <div className="mig-actions">
          {failed && (
            <button onClick={() => void install()} className="btn-primary btn-lg w-full">Try again</button>
          )}
          {failed && hasWorkingModel && (
            <button onClick={onDismiss} className="btn-ghost w-full">Continue with current model</button>
          )}
          {failed && !hasWorkingModel && (
            <button onClick={() => void useCloudForNow()} disabled={switching} className="btn-ghost w-full">
              <Cloud size={13} /> {switching ? "Switching…" : "Use cloud speech for now"}
            </button>
          )}
          {!failed && busy && hasWorkingModel && (
            <button onClick={onDismiss} className="btn-ghost w-full">Continue in background</button>
          )}
          {!failed && busy && !hasWorkingModel && !verifying && (
            <button onClick={() => void cancel()} className="btn-ghost w-full">Cancel</button>
          )}
        </div>
      </div>
    </div>
  );
}
