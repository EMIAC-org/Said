import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { ArrowRight, Check, Cpu, Download, Keyboard, Link, Loader2, Sparkles, X } from "lucide-react";
import {
  getPreferences, invoke, patchPreferences,
  getDesktopPrefs, setDesktopPrefs, requestBrowserAutomation,
} from "@/lib/invoke";
import { NEW_MODEL_FILE, NEW_MODEL_NAME, NEW_MODEL_SIZE_HINT } from "@/lib/onDeviceModel";
import type { ReclaimResult } from "@/components/ReclaimOldModelsRow";
import { friendlyError } from "@/lib/friendlyError";
import { ErrorNotice } from "./ErrorNotice";
import { HotkeyPicker } from "@/components/HotkeyPicker";
import { hotkeyDisplay, hotkeyMode, type Platform } from "@/lib/hotkeys";

interface ModelStatus {
  installed: boolean;
  size_bytes: number;
  path: string;
}

interface DownloadProgress {
  name: string;
  received: number;
  total: number;
  status: "downloading" | "done" | "cancelled" | "error" | string;
  error: string | null;
}

function formatSize(bytes: number): string {
  if (bytes >= 1e9) return `${(bytes / 1e9).toFixed(1)} GB`;
  if (bytes >= 1e6) return `${Math.round(bytes / 1e6)} MB`;
  if (bytes > 0) return `${Math.round(bytes / 1e3)} KB`;
  return "—";
}

/**
 * Forced post-update "Meet the new model" screen. Shown once to every
 * already-onboarded user (see lib/migration.ts) so a feature that ships via the
 * update pipeline — and therefore skips onboarding — is still 100% seen. The
 * user either installs the new model or explicitly keeps their current setup;
 * both dismiss the gate and stamp the migration version.
 */
export function ModelMigrationGate({
  onDone,
  platform,
}: {
  onDone: () => void;
  platform: Platform;
}) {
  const [gateStep, setGateStep] = useState<"model" | "hotkey" | "browser">("model");
  const [model, setModel] = useState<ModelStatus | null>(null);
  const [download, setDownload] = useState<DownloadProgress | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [recordHotkey, setRecordHotkey] = useState("caps_lock");
  const [modelChecked, setModelChecked] = useState(false);
  const mounted = useRef(true);
  const silentLegacyModelCleanupDone = useRef(false);

  const installed = model?.installed ?? false;

  useEffect(() => {
    void getPreferences().then((p) => {
      if (p && mounted.current) setRecordHotkey(p.record_hotkey || "caps_lock");
    });
  }, []);

  const saveHotkey = useCallback((id: string) => {
    setRecordHotkey(id);
    void patchPreferences({ record_hotkey: id }).catch(() => {});
  }, []);

  const refresh = useCallback(async () => {
    const s = await invoke<ModelStatus>("dictation_model_status").catch((e) => {
      if (mounted.current) setError(friendlyError(e));
      return null;
    });
    if (mounted.current) {
      setModel(s);
      setModelChecked(true);
    }
    return s;
  }, []);

  useEffect(() => {
    mounted.current = true;
    void refresh();
    return () => {
      mounted.current = false;
    };
  }, [refresh]);

  useEffect(() => {
    if (silentLegacyModelCleanupDone.current || !modelChecked) return;
    silentLegacyModelCleanupDone.current = true;
    void invoke<ReclaimResult>("reclaim_old_models")
      .then((result) => {
        if (mounted.current && result.removed.length > 0) void refresh();
      })
      .catch(() => {});
  }, [installed, modelChecked, refresh]);

  useEffect(() => {
    const un = listen<DownloadProgress>("meeting-model-download", (e) => {
      const p = e.payload;
      if (p.name !== NEW_MODEL_FILE) return;
      if (p.status === "downloading") {
        setDownload(p);
        setError("");
      } else {
        setDownload(null);
      }
      if (p.status === "done") void refresh();
      if (p.status === "error" && p.error) setError(friendlyError(p.error));
    });
    return () => {
      void un.then((f) => f());
    };
  }, [refresh]);

  const startDownload = useCallback(async () => {
    setBusy(true);
    setError("");
    try {
      await invoke("download_dictation_model");
      const status = await refresh();
      if (!status?.installed) {
        throw new Error(`${NEW_MODEL_NAME} did not install correctly.`);
      }
      await invoke<ReclaimResult>("reclaim_old_models").catch(() => null);
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      if (msg !== "cancelled") setError(friendlyError(msg));
    } finally {
      setBusy(false);
    }
  }, [refresh]);

  const cancelDownload = useCallback(async () => {
    await invoke("meeting_cancel_model_download", { name: NEW_MODEL_FILE }).catch(() => {});
    setDownload(null);
  }, []);

  const pct =
    download && download.total > 0
      ? Math.min(100, Math.round((download.received / download.total) * 100))
      : null;
  const checkingModel = !modelChecked;
  const downloading = pct !== null && !installed;
  const hk = hotkeyDisplay(recordHotkey, platform);
  const hkToggle = hotkeyMode(recordHotkey, platform) === "toggle";

  if (gateStep === "hotkey") {
    return (
      <div className="onb-error-screen">
        <div className="mig-card">
          <div className="mig-badge">
            <Keyboard size={12} /> Set your dictation key
          </div>
          <h2 className="mig-title">Pick your hotkey</h2>
          <p className="mig-desc">
            Hold this key anywhere to dictate — now you can choose any modifier, Caps Lock, or Fn.
            Press the key you want, or tap one below.
          </p>

          <div style={{ marginBottom: 18 }}>
            <HotkeyPicker value={recordHotkey} onChange={saveHotkey} platform={platform} />
          </div>

          <div className="mig-actions">
            <button
              onClick={() => (platform === "macos" ? setGateStep("browser") : onDone())}
              className="btn-primary btn-lg w-full"
            >
              {hkToggle ? `Tap ${hk.label} to dictate — done` : `Hold ${hk.label} to dictate — done`}
              <ArrowRight size={14} />
            </button>
          </div>
        </div>
      </div>
    );
  }

  // Browser context — macOS-only optional opt-in, announced in the forced update
  // flow so every user sees it. Enabling asks macOS for Automation consent.
  if (gateStep === "browser") {
    const enable = async () => {
      try {
        const p = await getDesktopPrefs();
        await setDesktopPrefs({ ...p, browser_context_enabled: true });
        void requestBrowserAutomation();
      } catch { /* best-effort */ }
      onDone();
    };
    return (
      <div className="onb-error-screen">
        <div className="mig-card">
          <div className="mig-badge">
            <Link size={12} /> New: browser context
          </div>
          <h2 className="mig-title">Smarter context per website</h2>
          <p className="mig-desc">
            AirNote can remember which website you’re dictating into — so it learns that Gmail,
            Twitter and your CMS each want a different style. It stores the domain only
            (e.g. mail.google.com, never the full URL), on this Mac. Enabling asks macOS for
            permission to read your browser’s active tab. Optional — change it anytime in Settings.
          </p>
          <div className="mig-actions">
            <button onClick={() => void enable()} className="btn-primary btn-lg w-full">
              Enable browser context
              <ArrowRight size={14} />
            </button>
          </div>
          <button type="button" onClick={onDone} className="onb-skip-link">
            Not now
          </button>
        </div>
      </div>
    );
  }

  return (
    <div className="onb-error-screen">
      <div className="mig-card">
        <div className="mig-badge">
          <Sparkles size={12} /> Local speech model
        </div>
        <h2 className="mig-title">Meet {NEW_MODEL_NAME}</h2>
        <p className="mig-desc">
          AirNote uses one on-device Hinglish model for dictation and meetings. It stays
          private, works offline, and has no per-use cost. Install it to enable local speech.
        </p>

        <div className="mig-model">
          <div className="mig-model-row">
            <div className="mig-model-left">
              <span className="mig-model-ico">
                <Cpu size={13} />
              </span>
              <span className="mig-model-name">
                {checkingModel
                  ? `Checking ${NEW_MODEL_NAME}…`
                  : installed
                  ? `${NEW_MODEL_NAME} · ${
                      model && model.size_bytes > 0 ? formatSize(model.size_bytes) : NEW_MODEL_SIZE_HINT
                    }`
                  : downloading
                    ? `Downloading ${NEW_MODEL_NAME}…`
                    : `${NEW_MODEL_NAME} · ${NEW_MODEL_SIZE_HINT}`}
              </span>
            </div>
            {checkingModel ? (
              <span className="mig-ready">
                <Loader2 size={12} className="animate-spin" /> Checking
              </span>
            ) : installed ? (
              <span className="mig-ready">
                <Check size={12} /> Installed
              </span>
            ) : downloading ? (
              <button
                type="button"
                onClick={() => void cancelDownload()}
                className="btn-ghost text-[11px] shrink-0"
                style={{ height: 26 }}
              >
                <X size={12} /> Cancel
              </button>
            ) : null}
          </div>

          {downloading && (
            <div className="mig-bar">
              <div style={{ width: `${Math.max(4, pct ?? 0)}%` }} />
            </div>
          )}

          <ErrorNotice error={error} onRetry={() => void startDownload()} className="mt-2" />

        </div>

        <div className="mig-actions">
          {installed ? (
            <button onClick={() => setGateStep("hotkey")} className="btn-primary btn-lg w-full">
              Next — pick your hotkey
              <ArrowRight size={14} />
            </button>
          ) : (
            <button
              onClick={() => void startDownload()}
              disabled={checkingModel || busy || downloading}
              className="btn-primary btn-lg w-full"
            >
              {checkingModel || busy || downloading ? (
                <Loader2 size={14} className="animate-spin" />
              ) : (
                <Download size={14} />
              )}
              {checkingModel
                ? "Checking local model…"
                : downloading
                ? `Installing… ${pct ?? 0}%`
                : `Install ${NEW_MODEL_NAME} · ${NEW_MODEL_SIZE_HINT}`}
            </button>
          )}
          <button type="button" onClick={() => setGateStep("hotkey")} className="onb-skip-link">
            Keep my current setup
          </button>
        </div>
      </div>
    </div>
  );
}
