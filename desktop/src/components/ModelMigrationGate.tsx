import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { ArrowRight, Check, Cloud, Cpu, Download, Loader2, Trash2 } from "lucide-react";
import {
  chooseInstalledLocalModel,
  getLocalModelInventory,
  getSttSetupPolicy,
  invoke,
  removeUnusedLocalDictationModels,
  type LocalModelInfo,
  type LocalModelInventory,
  type LocalModelKey,
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

function commandFor(model: LocalModelKey) {
  if (model === "nemotron-q4") {
    return {
      download: "download_nemotron_model",
      args: { variant: "q4" },
      event: "nemotron-model-download",
      eventName: "nemotron-3.5-asr-streaming-0.6b-Q4_K_M.gguf",
    };
  }
  return {
    download: "download_dictation_model",
    args: undefined,
    event: "meeting-model-download",
    eventName: "ggml-oriserve-hinglish-fp16.bin",
  };
}

function formatSize(bytes: number): string {
  if (bytes >= 1_000_000_000) return `${(bytes / 1_000_000_000).toFixed(1)} GB`;
  return `${Math.max(1, Math.round(bytes / 1_000_000))} MB`;
}

/**
 * Required speech-setup update. Existing users keep a verified working model
 * unless they explicitly upgrade. A replacement is selected only after its
 * downloader has finalized and native inventory verification succeeds.
 */
export function ModelMigrationGate({ onDone, platform: _platform }: { onDone: () => void; platform: Platform }) {
  const [policy, setPolicy] = useState<SttSetupPolicy | null>(null);
  const [inventory, setInventory] = useState<LocalModelInventory | null>(null);
  const [download, setDownload] = useState<DownloadProgress | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const mounted = useRef(true);

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
    const recommended = inventory?.recommended_model;
    if (!recommended || inventory?.setup_kind !== "local_required") return;
    const command = commandFor(recommended);
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
    return () => { void unlisten.then((stop) => stop()); };
  }, [inventory?.recommended_model, inventory?.setup_kind, refresh]);

  const activateModel = useCallback(async (model: LocalModelKey, finishAfter: boolean) => {
    setBusy(true);
    setError("");
    try {
      const next = await chooseInstalledLocalModel(model);
      if (mounted.current) setInventory(next);
      if (finishAfter || next.reclaimable_bytes === 0) onDone();
    } catch (cause) {
      setError(friendlyError(cause));
    } finally {
      setBusy(false);
    }
  }, [onDone]);

  const startUpgrade = useCallback(async () => {
    const recommended = inventory?.recommended_model;
    if (!recommended) return;
    const command = commandFor(recommended);
    setBusy(true);
    setError("");
    try {
      await invoke(command.download, command.args);
      const next = await chooseInstalledLocalModel(recommended);
      if (mounted.current) setInventory(next);
      if (next.reclaimable_bytes === 0) onDone();
    } catch (cause) {
      const message = friendlyError(cause);
      if (message.toLowerCase() !== "cancelled") setError(message);
    } finally {
      setBusy(false);
    }
  }, [inventory?.recommended_model, onDone]);

  const removeUnused = useCallback(async () => {
    setBusy(true);
    setError("");
    try {
      await removeUnusedLocalDictationModels();
      await refresh();
      onDone();
    } catch (cause) {
      setError(friendlyError(cause));
    } finally {
      setBusy(false);
    }
  }, [onDone, refresh]);

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
    const oriserve = inventory.models.find((model) => model.key === "oriserve");
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
            {oriserve?.installed && <p className="text-[11px] text-muted-foreground mt-2">Oriserve remains installed for local Meetings.</p>}
          </div>
          <div className="mig-actions"><button onClick={onDone} className="btn-primary btn-lg w-full">Continue <ArrowRight size={14} /></button></div>
        </div>
      </div>
    );
  }

  const recommended = inventory.models.find((model) => model.key === inventory.recommended_model) as LocalModelInfo | undefined;
  const existing = inventory.models.find((model) => model.key === inventory.existing_compatible_model);
  const recommendedActive = recommended?.active_for_dictation ?? false;
  const cleanupAvailable = recommendedActive && inventory.reclaimable_bytes > 0;
  const pct = download && download.total > 0 ? Math.min(100, Math.round((download.received / download.total) * 100)) : null;
  const downloading = pct !== null && !recommended?.installed;

  let title = `Install ${recommended?.name ?? "your local speech model"}.`;
  let description = `AirNote selected ${recommended?.name ?? "a local model"} for this Mac.`;
  if (cleanupAvailable) {
    title = "Your dictation upgrade is ready.";
    description = `${recommended?.name} is active. You can remove an unused older dictation model now or keep it as a rollback.`;
  } else if (recommended?.installed) {
    title = `${recommended.name} is already downloaded.`;
    description = existing
      ? `${existing.name} is currently working. Use the recommended model or continue with your existing one.`
      : "The recommended local dictation model is verified and ready to use.";
  } else if (existing) {
    title = `${existing.name} is already working.`;
    description = existing.key === "oriserve"
      ? `${recommended?.name} is recommended for dictation on this Mac. Oriserve will remain installed because Meetings use it.`
      : `${recommended?.name} is the balanced recommendation for this Mac. You can upgrade or continue with ${existing.name}.`;
  }

  return (
    <div className="mig-overlay" role="dialog" aria-modal="true" aria-labelledby="model-migration-title">
      <div className="mig-card">
        <div className="mig-badge"><Cpu size={12} /> Updated local speech setup</div>
        <h2 id="model-migration-title" className="mig-title">{title}</h2>
        <p className="mig-desc">{description}</p>
        <div className="mig-model" aria-live="polite">
          <div className="mig-model-row">
            <span className="mig-model-left"><span className="mig-model-ico"><Cpu size={13} /></span><span className="mig-model-name">{recommended?.name} · {recommended?.size_hint}</span></span>
            {recommended?.installed ? <span className="mig-ready"><Check size={12} /> Installed</span> : downloading ? <span className="mig-ready"><Loader2 size={12} className="animate-spin" /> {pct}%</span> : null}
          </div>
          {downloading && <div className="mig-bar"><div style={{ width: `${Math.max(4, pct ?? 0)}%` }} /></div>}
          {recommended?.key === "nemotron-q4" && inventory.models.find((model) => model.key === "oriserve")?.installed && (
            <p className="text-[11px] text-muted-foreground mt-2">Oriserve is protected for local Meetings and will not be removed by this upgrade.</p>
          )}
          <ErrorNotice error={error} onRetry={() => void startUpgrade()} className="mt-2" />
        </div>

        <div className="mig-actions">
          {cleanupAvailable ? (
            <>
              <button onClick={() => void removeUnused()} disabled={busy} className="btn-primary btn-lg w-full">
                {busy ? <Loader2 size={14} className="animate-spin" /> : <Trash2 size={14} />}
                Remove unused model · free {formatSize(inventory.reclaimable_bytes)}
              </button>
              <button onClick={onDone} disabled={busy} className="btn-ghost btn-lg w-full">Keep as rollback and continue</button>
            </>
          ) : recommended?.installed ? (
            <>
              <button onClick={() => void activateModel(recommended.key, false)} disabled={busy} className="btn-primary btn-lg w-full">
                {busy ? <Loader2 size={14} className="animate-spin" /> : <Check size={14} />}
                {recommendedActive ? "Continue" : `Use ${recommended.name}`}
              </button>
              {existing && existing.key !== recommended.key && (
                <button onClick={() => void activateModel(existing.key, true)} disabled={busy} className="btn-ghost btn-lg w-full">
                  Continue with {existing.name}
                </button>
              )}
            </>
          ) : (
            <>
              <button onClick={() => void startUpgrade()} disabled={busy || downloading} className="btn-primary btn-lg w-full">
                {busy || downloading ? <Loader2 size={14} className="animate-spin" /> : <Download size={14} />}
                {downloading ? `Installing… ${pct ?? 0}%` : `Upgrade to ${recommended?.name} · ${recommended?.size_hint}`}
              </button>
              {existing && (
                <button onClick={() => void activateModel(existing.key, true)} disabled={busy || downloading} className="btn-ghost btn-lg w-full">
                  Continue with {existing.name}
                </button>
              )}
            </>
          )}
        </div>
      </div>
    </div>
  );
}
