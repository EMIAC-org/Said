import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { ArrowRight, Check, Cloud, Cpu, Download, Loader2 } from "lucide-react";
import { getSttSetupPolicy, invoke, selectLocalDictationRoute, type SttSetupPolicy } from "@/lib/invoke";
import { friendlyError } from "@/lib/friendlyError";
import { ErrorNotice } from "./ErrorNotice";
import type { Platform } from "@/lib/hotkeys";

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

function commandFor(policy: SttSetupPolicy) {
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
    args: undefined,
    event: "meeting-model-download",
    eventName: "ggml-oriserve-hinglish-fp16.bin",
  };
}

/**
 * Required v6 speech-setup update. Unlike the former optional model card, this
 * cannot be dismissed until an Apple-Silicon Mac has the policy-selected local
 * model. Cloud-locked machines simply acknowledge their fixed live route.
 */
export function ModelMigrationGate({ onDone, platform: _platform }: { onDone: () => void; platform: Platform }) {
  const [policy, setPolicy] = useState<SttSetupPolicy | null>(null);
  const [model, setModel] = useState<ModelStatus | null>(null);
  const [download, setDownload] = useState<DownloadProgress | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const mounted = useRef(true);

  const refresh = useCallback(async () => {
    try {
      const next = await getSttSetupPolicy();
      if (!mounted.current) return null;
      setPolicy(next);
      if (next.setup_kind === "local_required") {
        const command = commandFor(next);
        const status = await invoke<ModelStatus>(command.status, command.args);
        if (mounted.current) setModel(status);
      }
      return next;
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
    if (!policy || policy.setup_kind !== "local_required") return;
    const command = commandFor(policy);
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
  }, [policy, refresh, selectLocalDictationRoute]);

  const startDownload = useCallback(async () => {
    if (!policy) return;
    const command = commandFor(policy);
    setBusy(true);
    setError("");
    try {
      await invoke(command.download, command.args);
      const status = await invoke<ModelStatus>(command.status, command.args);
      if (!status.installed) {
        throw new Error(`${policy.local_model_name} did not install correctly. Try again.`);
      }
      await selectLocalDictationRoute();
      setModel(status);
      await refresh();
    } catch (cause) {
      setError(friendlyError(cause));
    } finally {
      setBusy(false);
    }
  }, [policy, refresh]);

  const continueWithLocal = useCallback(async () => {
    setBusy(true);
    setError("");
    try {
      await selectLocalDictationRoute();
      onDone();
    } catch (cause) {
      setError(friendlyError(cause));
    } finally {
      setBusy(false);
    }
  }, [onDone]);

  if (!policy) {
    return (
      <div className="onb-error-screen">
        <div className="mig-card">
          {error ? (
            <>
              <div className="mig-badge"><Cpu size={12} /> Updated speech setup</div>
              <h2 className="mig-title">Couldn’t check this device.</h2>
              <p className="mig-desc">AirNote must determine the required speech setup before continuing.</p>
              <ErrorNotice error={error} onRetry={() => void refresh()} className="mt-3" />
              <div className="mig-actions"><button onClick={() => void refresh()} className="btn-primary btn-lg w-full">Try again</button></div>
            </>
          ) : <Loader2 className="animate-spin" size={18} />}
        </div>
      </div>
    );
  }

  if (policy.setup_kind === "cloud_locked") {
    const intelMac = policy.cpu_family === "intel";
    return (
      <div className="onb-error-screen">
        <div className="mig-card">
          <div className="mig-badge"><Cloud size={12} /> Updated speech setup</div>
          <h2 className="mig-title">Live Nemotron is enabled.</h2>
          <p className="mig-desc">
            {intelMac
              ? "This Intel Mac now uses AirNote’s live cloud speech engine for dictation."
              : "Windows now uses AirNote’s live cloud speech engine for dictation."}
            {" "}It streams while you speak and finalizes when you release your dictation key.
          </p>
          <div className="mig-model">
            <div className="mig-model-row">
              <span className="mig-model-left"><span className="mig-model-ico"><Cloud size={13} /></span><span className="mig-model-name">Nemotron Streaming 3.5 · Live</span></span>
              <span className="mig-ready"><Check size={12} /> Ready</span>
            </div>
          </div>
          <div className="mig-actions">
            <button onClick={onDone} className="btn-primary btn-lg w-full">Continue <ArrowRight size={14} /></button>
          </div>
        </div>
      </div>
    );
  }

  const installed = model?.installed ?? false;
  const pct = download && download.total > 0 ? Math.min(100, Math.round((download.received / download.total) * 100)) : null;
  const downloading = pct !== null && !installed;
  return (
    <div className="onb-error-screen">
      <div className="mig-card">
        <div className="mig-badge"><Cpu size={12} /> Updated local speech setup</div>
        <h2 className="mig-title">Install your local speech model.</h2>
        <p className="mig-desc">
          AirNote selected {policy.local_model_name} for this Apple Silicon Mac. Local dictation is required before continuing, so normal use does not incur cloud speech cost.
        </p>
        <div className="mig-model">
          <div className="mig-model-row">
            <span className="mig-model-left"><span className="mig-model-ico"><Cpu size={13} /></span><span className="mig-model-name">{policy.local_model_name} · {policy.local_model_size_hint}</span></span>
            {installed ? <span className="mig-ready"><Check size={12} /> Installed</span> : downloading ? <span className="mig-ready"><Loader2 size={12} className="animate-spin" /> {pct}%</span> : null}
          </div>
          {downloading && <div className="mig-bar"><div style={{ width: `${Math.max(4, pct ?? 0)}%` }} /></div>}
          <ErrorNotice error={error} onRetry={() => void startDownload()} className="mt-2" />
        </div>
        <div className="mig-actions">
          {installed ? (
            <button onClick={() => void continueWithLocal()} disabled={busy} className="btn-primary btn-lg w-full">
              {busy ? <Loader2 size={14} className="animate-spin" /> : "Continue"}
              {!busy && <ArrowRight size={14} />}
            </button>
          ) : (
            <button onClick={() => void startDownload()} disabled={busy || downloading} className="btn-primary btn-lg w-full">
              {busy || downloading ? <Loader2 size={14} className="animate-spin" /> : <Download size={14} />}
              {downloading ? `Installing… ${pct ?? 0}%` : `Install ${policy.local_model_name} · ${policy.local_model_size_hint}`}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
