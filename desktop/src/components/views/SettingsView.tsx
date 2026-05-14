import React, { useCallback, useEffect, useRef, useState } from "react";
import { ScrollArea } from "@/components/ui/scroll-area";
import { cn } from "@/lib/utils";
import {
  Shield, Cpu, Key, Info, Wifi, Check, Sparkles, Zap,
  Languages, MessageSquareText, Loader2, RefreshCw,
  Eye, EyeOff, Bell, Bug, Copy, FileText, Mic, Download,
} from "lucide-react";
import { check } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import type { AppSnapshot, Preferences } from "@/types";
import {
  getPreferences, patchPreferences,
  getDebugLogs,
  requestNotifications, checkNotificationPermission,
  type DebugLogs,
  type NotifPermission,
} from "@/lib/invoke";

// ── Tone presets ──────────────────────────────────────────────────────────────

const TONE_PRESETS = [
  { key: "neutral",      label: "Neutral",      desc: "Clear and balanced — no strong stylistic lean" },
  { key: "professional", label: "Professional",  desc: "Formal and polished — great for work emails" },
  { key: "casual",       label: "Casual",        desc: "Friendly and conversational — light and easy" },
  { key: "assertive",    label: "Assertive",     desc: "Direct and confident — strong calls-to-action" },
  { key: "concise",      label: "Concise",       desc: "Minimal words — every word earns its place" },
  { key: "custom",       label: "Custom",        desc: "Write your own persona instructions below" },
] as const;

type ToneKey = (typeof TONE_PRESETS)[number]["key"];

// ── Language options ──────────────────────────────────────────────────────────

const LANGUAGES = [
  { key: "auto",  label: "Auto (Hindi + English)" },
  { key: "hi",    label: "Hindi" },
  { key: "multi", label: "Hindi + English (code-switching)" },
  { key: "en",    label: "English" },
  { key: "en-IN", label: "English (India)" },
];

// ── Sub-components ────────────────────────────────────────────────────────────

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div className="mb-7">
      <p className="section-label px-1 mb-2.5 flex items-center gap-2">
        <span
          className="inline-block w-1 h-1 rounded-full"
          style={{ background: "hsl(var(--accent-violet))" }}
        />
        {title}
      </p>
      <div className="panel overflow-hidden">
        {children}
      </div>
    </div>
  );
}

function Row({
  icon, label, description, action,
}: {
  icon:         React.ReactNode;
  label:        string;
  description?: string;
  action?:      React.ReactNode;
  last?:        boolean;
}) {
  return (
    <div className="flex items-center gap-4 px-5 py-4">
      <div
        className="w-9 h-9 rounded-xl flex items-center justify-center flex-shrink-0"
        style={{
          background: "hsl(var(--surface-4))",
          color:      "hsl(var(--accent-violet))",
        }}
      >
        {icon}
      </div>
      <div className="flex-1 min-w-0">
        <p className="text-[13px] font-medium text-foreground">{label}</p>
        {description && (
          <p className="text-[12px] text-muted-foreground mt-0.5 leading-relaxed">{description}</p>
        )}
      </div>
      {action && <div className="flex-shrink-0 ml-4">{action}</div>}
    </div>
  );
}

// ── Section routing (used by SettingsModal) ───────────────────────────────────

export type SettingsSection =
  | "writing"
  | "permissions"
  | "api-keys"
  | "debug"
  | "about";

export const SETTINGS_SECTIONS: { id: SettingsSection; label: string }[] = [
  { id: "writing",     label: "Writing style" },
  { id: "permissions", label: "Permissions"   },
  { id: "api-keys",    label: "API keys"      },
  { id: "debug",       label: "Debug"         },
  { id: "about",       label: "About"         },
];

function Show({ when, children }: { when: boolean; children: React.ReactNode }) {
  return when ? <>{children}</> : null;
}

// ── Props ──────────────────────────────────────────────────────────────────────

interface SettingsViewProps {
  snapshot:          AppSnapshot | null;
  onAccessibility:   () => void;
  onInputMonitoring: () => void;
  onMicrophone:      () => void;
  /** When provided, only the matching section renders (modal mode). */
  activeSection?:    SettingsSection;
  /** Hide the page header entirely (modal mode renders its own). */
  hideHeader?:       boolean;
  /** Skip the page paddings + ScrollArea wrapper (modal already provides them). */
  embedded?:         boolean;
}

// ── View ───────────────────────────────────────────────────────────────────────

export function SettingsView({
  snapshot,
  onAccessibility,
  onInputMonitoring,
  onMicrophone,
  activeSection,
  hideHeader,
  embedded,
}: SettingsViewProps) {
  // Helper — true when the section should render (no filter = render all)
  const showAll = !activeSection;
  const isOn    = (id: SettingsSection) => showAll || activeSection === id;
  const axGranted  = snapshot?.accessibility_granted    ?? false;
  const imGranted  = snapshot?.input_monitoring_granted ?? false;
  const micGranted = snapshot?.microphone_granted       ?? false;

  const [notifPerm, setNotifPerm] = useState<NotifPermission>("unknown");
  const [notifBusy, setNotifBusy] = useState(false);
  const axSupported = snapshot?.auto_paste_supported    ?? false;

  // ── Prefs state ─────────────────────────────────────────────────────────────
  const [prefs,        setPrefs]        = useState<Preferences | null>(null);
  const [saving,       setSaving]       = useState(false);
  const [saveError,    setSaveError]    = useState("");
  const [customPrompt, setCustomPrompt] = useState("");
  const [promptDirty,  setPromptDirty]  = useState(false);

  // ── API key state ────────────────────────────────────────────────────────────
  const [gatewayKey,    setGatewayKey]    = useState("");
  const [deepgramKey,   setDeepgramKey]   = useState("");
  const [geminiKey,     setGeminiKey]     = useState("");
  const [groqKey,       setGroqKey]       = useState("");
  const [showGateway,   setShowGateway]   = useState(false);
  const [showDeepgram,  setShowDeepgram]  = useState(false);
  const [showGemini,    setShowGemini]    = useState(false);
  const [showGroq,      setShowGroq]      = useState(false);
  const [keySaving,     setKeySaving]     = useState(false);
  const [keySaved,      setKeySaved]      = useState(false);
  const keySaveTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  // ── Debug logs state ───────────────────────────────────────────────────────
  const [debugLogs,    setDebugLogs]    = useState<DebugLogs | null>(null);
  const [debugBusy,    setDebugBusy]    = useState(false);
  const [debugCopied,  setDebugCopied]  = useState<"combined" | "desktop" | "backend" | null>(null);
  const [debugTab,     setDebugTab]     = useState<"combined" | "desktop" | "backend">("combined");

  // ── Auto-update state ─────────────────────────────────────────────────────
  const [updateStatus, setUpdateStatus] = useState<"idle" | "checking" | "available" | "downloading" | "ready" | "up-to-date" | "error">("idle");
  const [updateVersion, setUpdateVersion] = useState("");
  const [updateError, setUpdateError] = useState("");

  const checkForUpdates = useCallback(async () => {
    setUpdateStatus("checking");
    setUpdateError("");
    try {
      const update = await check();
      if (update) {
        setUpdateVersion(update.version);
        setUpdateStatus("available");
      } else {
        setUpdateStatus("up-to-date");
      }
    } catch (err) {
      setUpdateError(err instanceof Error ? err.message : String(err));
      setUpdateStatus("error");
    }
  }, []);

  const downloadAndInstall = useCallback(async () => {
    setUpdateStatus("downloading");
    try {
      const update = await check();
      if (!update) return;
      await update.downloadAndInstall();
      setUpdateStatus("ready");
    } catch (err) {
      setUpdateError(err instanceof Error ? err.message : String(err));
      setUpdateStatus("error");
    }
  }, []);
  const recordHotkey = prefs?.record_hotkey ?? "caps_lock";
  const recordHotkeyLabel =
    recordHotkey === "right_option" ? "Right Option" :
    recordHotkey === "fn" ? "Fn" :
    "Caps Lock";

  function syncApiKeyInputs(nextPrefs: Preferences) {
    setGatewayKey(nextPrefs.gateway_api_key ?? "");
    setDeepgramKey(nextPrefs.deepgram_api_key ?? "");
    setGeminiKey(nextPrefs.gemini_api_key ?? "");
    setGroqKey(nextPrefs.groq_api_key ?? "");
    setShowGateway(false);
    setShowDeepgram(false);
    setShowGemini(false);
    setShowGroq(false);
  }

  useEffect(() => {
    let alive = true;
    const refresh = () => {
      checkNotificationPermission().then((p) => {
        if (alive) setNotifPerm(p);
      });
    };
    refresh();
    // Re-check when the user comes back to the window (after toggling perms
    // in System Settings) or when the tab becomes visible again
    window.addEventListener("focus",            refresh);
    document.addEventListener("visibilitychange", refresh);
    return () => {
      alive = false;
      window.removeEventListener("focus",            refresh);
      document.removeEventListener("visibilitychange", refresh);
    };
  }, []);

  // Permissions section "Allow / Open Settings" handler — requests notification
  // permission. macOS only shows the prompt once; if denied, the user must
  // toggle it in System Settings.
  async function handleNotifTest() {
    setNotifBusy(true);
    try {
      const current = await checkNotificationPermission();
      if (current === "granted") {
        setNotifPerm("granted");
        return;
      }
      const result = await requestNotifications();
      setNotifPerm(result);
    } finally {
      setNotifBusy(false);
    }
  }

  useEffect(() => {
    getPreferences().then((p) => {
      if (p) {
        setPrefs(p);
        setCustomPrompt(p.custom_prompt ?? "");
        syncApiKeyInputs(p);
      }
    });
  }, []);

  useEffect(() => {
    return () => {
      if (keySaveTimer.current) clearTimeout(keySaveTimer.current);
    };
  }, []);

  async function refreshDebugLogs() {
    setDebugBusy(true);
    try {
      setDebugLogs(await getDebugLogs());
    } finally {
      setDebugBusy(false);
    }
  }

  async function copyDebugLog(kind: "combined" | "desktop" | "backend") {
    const text = debugLogs?.[kind] ?? "";
    if (!text.trim()) return;
    await navigator.clipboard.writeText(text);
    setDebugCopied(kind);
    setTimeout(() => setDebugCopied((prev) => prev === kind ? null : prev), 1800);
  }

  async function saveApiKeys() {
    if (!prefs) return;
    setKeySaving(true);
    setSaveError("");
    try {
      const update: Partial<Preferences> = {};
      const currentGateway = prefs.gateway_api_key ?? "";
      const currentDeepgram = prefs.deepgram_api_key ?? "";
      const currentGemini = prefs.gemini_api_key ?? "";
      const currentGroq = prefs.groq_api_key ?? "";
      const nextGateway = gatewayKey.trim();
      const nextDeepgram = deepgramKey.trim();
      const nextGemini = geminiKey.trim();
      const nextGroq = groqKey.trim();

      if (nextGateway !== currentGateway) update.gateway_api_key = nextGateway || null;
      if (nextDeepgram !== currentDeepgram) update.deepgram_api_key = nextDeepgram || null;
      if (nextGemini !== currentGemini) update.gemini_api_key = nextGemini || null;
      if (nextGroq !== currentGroq) update.groq_api_key = nextGroq || null;
      if (prefs.learning_enabled && currentGemini !== "" && nextGemini === "") {
        update.learning_enabled = false;
      }

      if (Object.keys(update).length === 0) {
        setKeySaved(true);
        if (keySaveTimer.current) clearTimeout(keySaveTimer.current);
        keySaveTimer.current = setTimeout(() => setKeySaved(false), 2500);
        return;
      }

      const updated = await patchPreferences(update);
      if (!updated) throw new Error("preferences update returned no data");
      if (updated) {
        setPrefs(updated);
        syncApiKeyInputs(updated);
      }
      setKeySaved(true);
      if (keySaveTimer.current) clearTimeout(keySaveTimer.current);
      keySaveTimer.current = setTimeout(() => setKeySaved(false), 2500);
    } catch (err) {
      setSaveError(err instanceof Error ? err.message : "Failed to save — is the backend running?");
    } finally {
      setKeySaving(false);
    }
  }

  async function patch(update: Partial<Preferences>) {
    if (!prefs) return;
    setSaving(true);
    setSaveError("");
    try {
      const updated = await patchPreferences(update);
      if (updated) setPrefs(updated);
    } catch (err) {
      console.error("[patch] error:", err);
      setSaveError("Failed to save — is the backend running?");
    } finally {
      setSaving(false);
    }
  }

  async function saveCustomPrompt() {
    await patch({ custom_prompt: customPrompt || null });
    setPromptDirty(false);
  }

  useEffect(() => {
    if (isOn("debug") && !debugLogs && !debugBusy) {
      refreshDebugLogs();
    }
  }, [activeSection]);

  const tone = (prefs?.tone_preset ?? "neutral") as ToneKey;
  const hasStoredGeminiKey = Boolean(prefs?.gemini_api_key);
  const learningEnabled = prefs?.learning_enabled ?? true;

  // Inner content that gets either wrapped in ScrollArea (full view) or rendered
  // bare (modal embeds it inside its own scroll container).
  const inner = (
    <>

        {/* ── Header ───────────────────────────────────── */}
        <Show when={!hideHeader}>
        <div className="mb-6 flex items-end justify-between gap-4">
          <div>
            <h1 className="text-[24px] font-bold tracking-tight text-foreground leading-tight">
              Settings
            </h1>
            <p className="text-[12.5px] text-muted-foreground mt-1 flex items-center gap-2">
              <span
                className="inline-block w-1.5 h-1.5 rounded-full"
                style={{
                  background: saving ? "hsl(var(--accent-violet))" : "hsl(var(--primary))",
                  boxShadow:  saving
                    ? "0 0 8px hsl(var(--accent-violet) / 0.6)"
                    : "0 0 8px hsl(var(--primary) / 0.5)",
                }}
              />
              {saving ? "Saving preferences…" : "Preferences saved automatically"}
            </p>
          </div>
          {saving && (
            <div className="flex items-center gap-1.5 text-xs text-muted-foreground mb-1">
              <Loader2 size={13} className="animate-spin" />
              Saving…
            </div>
          )}
          {saveError && (
            <p className="text-xs mb-1" style={{ color: "hsl(354 78% 60%)" }}>{saveError}</p>
          )}
        </div>
        </Show>

        {/* ── Tone & Persona ───────────────────────────── */}
        <Show when={isOn("writing")}>
        <div className="mb-7">
          <p className="section-label px-1 mb-2.5">Writing Style</p>

          {/* Tone pill grid */}
          <div className="panel p-4 mb-3">
            <p className="text-[12px] font-semibold text-foreground mb-3">Tone Preset</p>
            <div className="grid grid-cols-3 gap-2">
              {TONE_PRESETS.map((t) => {
                const isActive = tone === t.key;
                return (
                  <button
                    key={t.key}
                    onClick={() => patch({ tone_preset: t.key })}
                    className="text-left px-3 py-2.5 rounded-xl transition-all"
                    style={{
                      background: isActive
                        ? "hsl(var(--surface-4))"
                        : "hsl(var(--surface-4))",
                      color: isActive
                        ? "hsl(var(--muted-foreground))"
                        : "hsl(var(--muted-foreground))",
                    }}
                  >
                    <p className="text-[12px] font-semibold leading-tight">{t.label}</p>
                    <p className="text-[10px] leading-snug mt-0.5 opacity-70">{t.desc}</p>
                  </button>
                );
              })}
            </div>
          </div>

          {/* Custom persona textarea */}
          <div className={cn("panel p-4 transition-all", tone !== "custom" && "opacity-60")}>
            <div className="flex items-center gap-2 mb-2">
              <MessageSquareText size={14} className="text-muted-foreground" />
              <p className="text-[12px] font-semibold text-foreground">Custom Persona Instructions</p>
              {tone !== "custom" && (
                <span className="text-[10px] text-muted-foreground ml-auto">
                  Select "Custom" above to activate
                </span>
              )}
            </div>
            <textarea
              value={customPrompt}
              onChange={(e) => { setCustomPrompt(e.target.value); setPromptDirty(true); }}
              onBlur={() => { if (promptDirty) saveCustomPrompt(); }}
              placeholder={
                'e.g. "You are a direct, no-nonsense communicator. Use bullet points where possible."'
              }
              rows={4}
              disabled={tone !== "custom"}
              className={cn(
                "input resize-none leading-relaxed transition-opacity",
                tone !== "custom" && "cursor-not-allowed"
              )}
            />
            {promptDirty && tone === "custom" && (
              <div className="flex items-center justify-end mt-2 gap-2">
                <button
                  onClick={() => { setCustomPrompt(prefs?.custom_prompt ?? ""); setPromptDirty(false); }}
                  className="text-[12px] text-muted-foreground hover:text-foreground transition-colors"
                >
                  Cancel
                </button>
                <button onClick={saveCustomPrompt} className="btn-primary !py-1.5 !px-3 !text-[12px]">
                  Save
                </button>
              </div>
            )}
          </div>
        </div>

        {/* ── Language ─────────────────────────────────── */}
        <Section title="Language">
          {/* Output language toggle */}
          <div className="px-5 pt-4 pb-3">
            <div className="flex items-center gap-4">
              <div
                className="w-9 h-9 rounded-xl flex items-center justify-center flex-shrink-0 text-muted-foreground"
                style={{ background: "hsl(var(--surface-4))" }}
              >
                <Languages size={16} />
              </div>
              <div className="flex-1">
                <p className="text-[13px] font-medium text-foreground mb-0.5">Output Language</p>
                <p className="text-[12px] text-muted-foreground">
                  What language the polished text is written in
                </p>
              </div>
            </div>
            {/* Three-way pill toggle */}
            <div
              className="flex mt-3 rounded-xl p-0.5 gap-0.5"
              style={{ background: "hsl(var(--surface-4))" }}
            >
              {(["hinglish", "hindi", "english"] as const).map((opt) => {
                const label = opt === "hinglish" ? "Hinglish" : opt === "hindi" ? "हिंदी" : "English";
                const isActive = (prefs?.output_language ?? "hinglish") === opt;
                return (
                  <button
                    key={opt}
                    onClick={() => patch({ output_language: opt })}
                    className="flex-1 text-[13px] font-medium rounded-[10px] py-1.5 transition-all"
                    style={{
                      background: isActive ? "hsl(var(--surface-1))" : "transparent",
                      color: isActive ? "hsl(var(--foreground))" : "hsl(var(--muted-foreground))",
                      boxShadow: isActive ? "0 1px 3px rgba(0,0,0,0.25)" : "none",
                    }}
                  >
                    {label}
                  </button>
                );
              })}
            </div>
          </div>

          <div className="mx-5 border-t" style={{ borderColor: "hsl(var(--surface-3))" }} />

          {/* Transcription language */}
          <div className="px-5 py-4">
            <div className="flex items-center gap-4">
              <div
                className="w-9 h-9 rounded-xl flex items-center justify-center flex-shrink-0 text-muted-foreground"
                style={{ background: "hsl(var(--surface-4))" }}
              >
                <Languages size={16} />
              </div>
              <div className="flex-1">
                <p className="text-[13px] font-medium text-foreground mb-1">Transcription Language</p>
                <p className="text-[12px] text-muted-foreground">
                  Use "Auto" for mixed Hindi + English, or "code-switching" to force multilingual recognition
                </p>
              </div>
              <select
                value={prefs?.language ?? "auto"}
                onChange={(e) => patch({ language: e.target.value })}
                className="text-[13px] rounded-lg px-3 py-1.5 cursor-pointer focus:outline-none"
                style={{
                  background: "hsl(var(--surface-4))",
                  color: "hsl(var(--foreground))",
                  border: "none",
                }}
              >
                {LANGUAGES.map((l) => (
                  <option key={l.key} value={l.key}>{l.label}</option>
                ))}
              </select>
            </div>
          </div>

          <div className="mx-5 border-t" style={{ borderColor: "hsl(var(--surface-3))" }} />

          <div className="px-5 py-4">
            <div className="flex items-center gap-4">
              <div
                className="w-9 h-9 rounded-xl flex items-center justify-center flex-shrink-0 text-muted-foreground"
                style={{ background: "hsl(var(--surface-4))" }}
              >
                <Key size={16} />
              </div>
              <div className="flex-1">
                <p className="text-[13px] font-medium text-foreground mb-1">Recording Hotkey</p>
                <p className="text-[12px] text-muted-foreground">
                  Choose which key you hold to start recording.
                </p>
              </div>
            </div>
            <div
              className="flex mt-3 rounded-xl p-0.5 gap-0.5"
              style={{ background: "hsl(var(--surface-4))" }}
            >
              {([
                { key: "caps_lock", label: "Caps Lock" },
                { key: "right_option", label: "Right Option" },
                { key: "fn", label: "Fn" },
              ] as const).map((opt) => {
                const isActive = recordHotkey === opt.key;
                return (
                  <button
                    key={opt.key}
                    onClick={() => patch({ record_hotkey: opt.key })}
                    className="flex-1 text-[13px] font-medium rounded-[10px] py-1.5 transition-all"
                    style={{
                      background: isActive ? "hsl(var(--surface-1))" : "transparent",
                      color: isActive ? "hsl(var(--foreground))" : "hsl(var(--muted-foreground))",
                      boxShadow: isActive ? "0 1px 3px rgba(0,0,0,0.25)" : "none",
                    }}
                  >
                    {opt.label}
                  </button>
                );
              })}
            </div>
          </div>
        </Section>
        </Show>

        {/* ── Permissions ──────────────────────────────── */}
        <Show when={isOn("permissions")}>
        <div className="mb-7">
          <p className="section-label px-1 mb-2.5">Permissions</p>

          {/* Combined info banner when any permission is missing */}
          {axSupported && (!micGranted || !axGranted || !imGranted) && (
            <div
              className="rounded-xl px-4 py-3 mb-3 text-[12px] leading-relaxed"
              style={{ background: "hsl(38 80% 12%)", color: "hsl(38 90% 70%)" }}
            >
              <p className="font-semibold mb-1">Permissions needed</p>
              {!micGranted && (
                <p>• <strong>Microphone</strong> — lets Said record your voice.</p>
              )}
              {!axGranted && (
                <p>• <strong>Accessibility</strong> — lets Said paste text directly into any app.</p>
              )}
              {!imGranted && (
                <p>• <strong>Input Monitoring</strong> — lets Said listen for the {recordHotkeyLabel} recording hotkey.</p>
              )}
              <p className="mt-1.5 opacity-70">
                After granting a permission, return to Said. This page updates automatically.
              </p>
            </div>
          )}

          <div className="panel overflow-hidden">
            {/* Row 1: Accessibility */}
            <div className="flex items-center gap-4 px-5 py-4">
              <div
                className="w-9 h-9 rounded-xl flex items-center justify-center flex-shrink-0"
                style={{ background: "hsl(var(--surface-4))", color: "hsl(var(--muted-foreground))" }}
              >
                <Mic size={16} />
              </div>
              <div className="flex-1 min-w-0">
                <p className="text-[13px] font-medium text-foreground">Microphone</p>
                <p className="text-[12px] text-muted-foreground mt-0.5 leading-relaxed">
                  {micGranted
                    ? "Granted — Said can record your voice."
                    : "Required for dictation. macOS will ask once, then use System Settings if denied."}
                </p>
              </div>
              <div className="flex-shrink-0 ml-4">
                {micGranted ? (
                  <span
                    className="text-[12px] font-semibold px-3 py-1.5 rounded-lg flex items-center gap-1"
                    style={{ background: "hsl(var(--surface-4))", color: "hsl(var(--muted-foreground))" }}
                  >
                    <Check size={11} /> Granted
                  </span>
                ) : (
                  <button
                    onClick={onMicrophone}
                    className="text-[12px] font-semibold px-3 py-1.5 rounded-lg transition-colors"
                    style={{ background: "hsl(var(--primary))", color: "hsl(var(--primary-foreground))" }}
                  >
                    Allow
                  </button>
                )}
              </div>
            </div>

            <div className="mx-5 border-t" style={{ borderColor: "hsl(var(--surface-3))" }} />

            {/* Row 2: Accessibility */}
            <div className="flex items-center gap-4 px-5 py-4">
              <div
                className="w-9 h-9 rounded-xl flex items-center justify-center flex-shrink-0"
                style={{
                  background: axGranted
                    ? "hsl(var(--surface-4))"
                    : "hsl(var(--surface-4))",
                  color: axGranted
                    ? "hsl(var(--muted-foreground))"
                    : "hsl(var(--muted-foreground))",
                }}
              >
                <Shield size={16} />
              </div>
              <div className="flex-1 min-w-0">
                <p className="text-[13px] font-medium text-foreground">Accessibility</p>
                <p className="text-[12px] text-muted-foreground mt-0.5 leading-relaxed">
                  {axGranted
                    ? "Granted — Said can paste text into any app."
                    : "Required for auto-paste. Opens System Settings → Privacy & Security → Accessibility."}
                </p>
              </div>
              <div className="flex-shrink-0 ml-4">
                {axSupported ? (
                  axGranted ? (
                    <span
                      className="text-[12px] font-semibold px-3 py-1.5 rounded-lg flex items-center gap-1"
                      style={{ background: "hsl(var(--surface-4))", color: "hsl(var(--muted-foreground))" }}
                    >
                      <Check size={11} /> Granted
                    </span>
                  ) : (
                    <button
                      onClick={onAccessibility}
                      className="text-[12px] font-semibold px-3 py-1.5 rounded-lg transition-colors"
                      style={{ background: "hsl(var(--primary))", color: "hsl(var(--primary-foreground))" }}
                    >
                      Open Settings
                    </button>
                  )
                ) : (
                  <span className="text-[12px] text-muted-foreground">macOS only</span>
                )}
              </div>
            </div>

            {/* Divider */}
            <div className="mx-5 border-t" style={{ borderColor: "hsl(var(--surface-3))" }} />

            {/* Row 3: Notifications */}
            <div className="flex items-center gap-4 px-5 py-4">
              <div
                className="w-9 h-9 rounded-xl flex items-center justify-center flex-shrink-0"
                style={{
                  background: notifPerm === "granted"
                    ? "hsl(var(--surface-4))"
                    : "hsl(var(--surface-4))",
                  color: notifPerm === "granted"
                    ? "hsl(var(--muted-foreground))"
                    : "hsl(var(--muted-foreground))",
                }}
              >
                <Bell size={16} />
              </div>
              <div className="flex-1 min-w-0">
                <p className="text-[13px] font-medium text-foreground">Notifications</p>
                <p className="text-[12px] text-muted-foreground mt-0.5 leading-relaxed">
                  {notifPerm === "granted"
                    ? "Granted — Said will notify you when a learning edit is ready to review."
                    : notifPerm === "denied"
                    ? "Denied — open System Settings → Notifications → Said to enable."
                    : "Said asks once to send learning-edit notifications."}
                </p>
              </div>
              <div className="flex-shrink-0 ml-4">
                {axSupported ? (
                  notifPerm === "granted" ? (
                    <span
                      className="text-[12px] font-semibold px-3 py-1.5 rounded-lg flex items-center gap-1"
                      style={{ background: "hsl(var(--surface-4))", color: "hsl(var(--muted-foreground))" }}
                    >
                      <Check size={11} /> Granted
                    </span>
                  ) : (
                    <button
                      disabled={notifBusy}
                      onClick={handleNotifTest}
                      className="text-[12px] font-semibold px-3 py-1.5 rounded-lg transition-colors flex items-center gap-1.5 disabled:opacity-50"
                      style={{ background: "hsl(var(--primary))", color: "hsl(var(--primary-foreground))" }}
                    >
                      {notifBusy && <Loader2 size={11} className="animate-spin" />}
                      {notifPerm === "denied" ? "Open Settings" : "Allow"}
                    </button>
                  )
                ) : (
                  <span className="text-[12px] text-muted-foreground">macOS only</span>
                )}
              </div>
            </div>

            {/* Divider */}
            <div className="mx-5 border-t" style={{ borderColor: "hsl(var(--surface-3))" }} />

            {/* Row 4: Input Monitoring */}
            <div className="flex items-center gap-4 px-5 py-4">
              <div
                className="w-9 h-9 rounded-xl flex items-center justify-center flex-shrink-0"
                style={{
                  background: imGranted
                    ? "hsl(var(--surface-4))"
                    : "hsl(var(--surface-4))",
                  color: imGranted
                    ? "hsl(var(--muted-foreground))"
                    : "hsl(var(--muted-foreground))",
                }}
              >
                <Key size={16} />
              </div>
              <div className="flex-1 min-w-0">
                <p className="text-[13px] font-medium text-foreground">Input Monitoring</p>
                <p className="text-[12px] text-muted-foreground mt-0.5 leading-relaxed">
                  {imGranted
                    ? `Granted — ${recordHotkeyLabel} recording hotkey is active.`
                    : `Required for the ${recordHotkeyLabel} recording hotkey to work. Opens System Settings → Privacy & Security → Input Monitoring.`}
                </p>
              </div>
              <div className="flex-shrink-0 ml-4">
                {axSupported ? (
                  imGranted ? (
                    <span
                      className="text-[12px] font-semibold px-3 py-1.5 rounded-lg flex items-center gap-1"
                      style={{ background: "hsl(var(--surface-4))", color: "hsl(var(--muted-foreground))" }}
                    >
                      <Check size={11} /> Granted
                    </span>
                  ) : (
                    <button
                      onClick={onInputMonitoring}
                      className="text-[12px] font-semibold px-3 py-1.5 rounded-lg transition-colors"
                      style={{ background: "hsl(var(--primary))", color: "hsl(var(--primary-foreground))" }}
                    >
                      Open Settings
                    </button>
                  )
                ) : (
                  <span className="text-[12px] text-muted-foreground">macOS only</span>
                )}
              </div>
            </div>

          </div>
        </div>
        </Show>

        {/* ── API Keys ──────────────────────────────────── */}
        <Show when={isOn("api-keys")}>
        <div className="mb-7">
          <p className="section-label px-1 mb-2.5">API Keys</p>
          <div className="panel p-5 space-y-4">
            <p className="text-[12px] text-muted-foreground leading-relaxed">
              Keys are loaded from the local SQLite database and stored only on this Mac.
            </p>

            {/* Gateway API Key */}
            <div>
              <p className="text-[12px] font-semibold text-foreground mb-1.5 flex items-center gap-1.5">
                <Wifi size={12} className="text-muted-foreground" />
                Gateway API Key
              </p>
              <div className="relative">
                <input
                  type={showGateway ? "text" : "password"}
                  placeholder="sk-…"
                  value={gatewayKey}
                  onChange={(e) => setGatewayKey(e.target.value)}
                  className="input pr-9 font-mono text-[12px]"
                />
                <button
                  type="button"
                  onClick={() => setShowGateway((v) => !v)}
                  className="absolute right-2.5 top-1/2 -translate-y-1/2 text-muted-foreground hover:text-foreground transition-colors"
                  tabIndex={-1}
                >
                  {showGateway ? <EyeOff size={14} /> : <Eye size={14} />}
                </button>
              </div>
            </div>

            {/* Deepgram API Key */}
            <div>
              <p className="text-[12px] font-semibold text-foreground mb-1.5 flex items-center gap-1.5">
                <Cpu size={12} className="text-muted-foreground" />
                Deepgram API Key
              </p>
              <div className="relative">
                <input
                  type={showDeepgram ? "text" : "password"}
                  placeholder="Token …"
                  value={deepgramKey}
                  onChange={(e) => setDeepgramKey(e.target.value)}
                  className="input pr-9 font-mono text-[12px]"
                />
                <button
                  type="button"
                  onClick={() => setShowDeepgram((v) => !v)}
                  className="absolute right-2.5 top-1/2 -translate-y-1/2 text-muted-foreground hover:text-foreground transition-colors"
                  tabIndex={-1}
                >
                  {showDeepgram ? <EyeOff size={14} /> : <Eye size={14} />}
                </button>
              </div>
            </div>

            {/* Gemini API Key */}
            <div>
              <div className="mb-1.5">
                <p className="text-[12px] font-semibold text-foreground flex items-center gap-1.5">
                  <Sparkles size={12} className="text-muted-foreground" />
                  Gemini API Key
                </p>
                <p className="text-[11px] text-muted-foreground mt-0.5">
                  Optional — enables smart learning
                </p>
              </div>
              <div className="relative">
                <input
                  type={showGemini ? "text" : "password"}
                  placeholder="AIza…"
                  value={geminiKey}
                  onChange={(e) => setGeminiKey(e.target.value)}
                  className="input pr-9 font-mono text-[12px]"
                />
                <button
                  type="button"
                  onClick={() => setShowGemini((v) => !v)}
                  className="absolute right-2.5 top-1/2 -translate-y-1/2 text-muted-foreground hover:text-foreground transition-colors"
                  tabIndex={-1}
                >
                  {showGemini ? <EyeOff size={14} /> : <Eye size={14} />}
                </button>
              </div>

              <div className="mt-3 flex items-center justify-between gap-4 rounded-xl px-3 py-2.5"
                   style={{ background: "hsl(var(--surface-3))" }}>
                <div className="min-w-0">
                  <p className="text-[12px] font-semibold text-foreground">Enable smart learning</p>
                  <p className="text-[11px] text-muted-foreground mt-0.5">
                    Uses Gemini embeddings to remember corrected words and context.
                  </p>
                </div>
                <button
                  type="button"
                  role="switch"
                  aria-checked={learningEnabled}
                  disabled={!hasStoredGeminiKey}
                  title={!hasStoredGeminiKey ? "Enter a Gemini key above to enable" : undefined}
                  onClick={() => patch({ learning_enabled: !learningEnabled })}
                  className="relative h-6 w-11 rounded-full transition-colors disabled:cursor-not-allowed disabled:opacity-50"
                  style={{
                    background: learningEnabled && hasStoredGeminiKey
                      ? "hsl(var(--primary))"
                      : "hsl(var(--surface-4))",
                  }}
                >
                  <span
                    className="absolute top-1 h-4 w-4 rounded-full transition-transform"
                    style={{
                      left: 4,
                      transform: learningEnabled && hasStoredGeminiKey
                        ? "translateX(20px)"
                        : "translateX(0)",
                      background: "hsl(var(--foreground))",
                    }}
                  />
                </button>
              </div>
            </div>

            {/* Groq API Key */}
            <div>
              <p className="text-[12px] font-semibold text-foreground mb-1.5 flex items-center gap-1.5">
                <Zap size={12} className="text-muted-foreground" />
                Groq API Key
                <span className="ml-1 px-1.5 py-0.5 rounded text-[10px] font-medium"
                      style={{ background: "hsl(var(--surface-4))", color: "hsl(var(--muted-foreground))" }}>
                  Fast
                </span>
              </p>
              <p className="text-[11px] text-muted-foreground mb-1.5">
                Get a free key at <span className="font-medium">console.groq.com</span> — enables Groq LPU provider (llama-3.3-70b, ~200ms TTFT)
              </p>
              <div className="relative">
                <input
                  type={showGroq ? "text" : "password"}
                  placeholder="gsk_…"
                  value={groqKey}
                  onChange={(e) => setGroqKey(e.target.value)}
                  className="input pr-9 font-mono text-[12px]"
                />
                <button
                  type="button"
                  onClick={() => setShowGroq((v) => !v)}
                  className="absolute right-2.5 top-1/2 -translate-y-1/2 text-muted-foreground hover:text-foreground transition-colors"
                  tabIndex={-1}
                >
                  {showGroq ? <EyeOff size={14} /> : <Eye size={14} />}
                </button>
              </div>
            </div>

            {/* Save button */}
            <div className="flex items-center justify-between pt-1">
              {saveError && (
                <p className="text-[12px]" style={{ color: "hsl(0 75% 75%)" }}>{saveError}</p>
              )}
              <div className="ml-auto flex items-center gap-3">
                {keySaved && (
                  <span className="text-[12px] flex items-center gap-1" style={{ color: "hsl(var(--muted-foreground))" }}>
                    <Check size={12} /> Saved
                  </span>
                )}
                <button
                  onClick={saveApiKeys}
                  disabled={keySaving}
                  className="btn-primary !py-1.5 !px-4 !text-[12px] flex items-center gap-1.5"
                >
                  {keySaving ? <Loader2 size={12} className="animate-spin" /> : null}
                  Save Keys
                </button>
              </div>
            </div>
          </div>
        </div>

        {/* ── LLM Provider picker ───────────────────────── */}
        <Section title="LLM Provider">
          {/* Provider option list */}
          {([
            {
              id:    "groq",
              icon:  <Zap size={15} />,
              label: "Groq LPU",
              desc:  "Llama 4 Scout — fastest (~200ms TTFT), free tier",
              badge: "Default",
              needsKey: !prefs?.groq_api_key && !groqKey,
            },
            {
              id:    "gemini_direct",
              icon:  <Sparkles size={15} />,
              label: "Gemini Direct",
              desc:  "gemini-2.0-flash-thinking via Google AI — needs Gemini API key",
              badge: null,
              needsKey: !prefs?.gemini_api_key && !geminiKey,
            },
          ] as const).map((opt, idx, arr) => {
            const isActive = prefs?.llm_provider === opt.id;
            return (
              <Row
                key={opt.id}
                icon={opt.icon}
                label={opt.label}
                description={opt.desc}
                last={idx === arr.length - 1}
                action={
                  <div className="flex items-center gap-2">
                    {opt.needsKey && (
                      <span className="text-[10px] px-1.5 py-0.5 rounded"
                            style={{ background: "hsl(30 80% 20%)", color: "hsl(30 90% 75%)" }}>
                        Key missing
                      </span>
                    )}
                    {opt.badge && !isActive && (
                      <span className="badge-model">{opt.badge}</span>
                    )}
                    <button
                      onClick={() => void patch({ llm_provider: opt.id })}
                      className={`px-3 py-1 rounded-md text-[11px] font-medium transition-all border ${
                        isActive
                          ? "border-transparent text-background"
                          : "border-border text-muted-foreground hover:text-foreground hover:border-foreground/30"
                      }`}
                      style={isActive ? { background: "hsl(var(--muted-foreground))" } : {}}
                    >
                      {isActive ? "✓ Active" : "Use"}
                    </button>
                  </div>
                }
              />
            );
          })}
        </Section>
        </Show>

        {/* ── Debug ───────────────────────────────────── */}
        <Show when={isOn("debug")}>
        <div className="mb-7">
          <div className="flex items-center justify-between px-1 mb-2.5">
            <p className="section-label flex items-center gap-2">
              <span
                className="inline-block w-1 h-1 rounded-full"
                style={{ background: "hsl(var(--accent-violet))" }}
              />
              Runtime Logs
            </p>
            <div className="flex items-center gap-2">
              {debugLogs?.truncated && (
                <span className="text-[10px] px-2 py-1 rounded-md"
                      style={{ background: "hsl(var(--surface-4))", color: "hsl(var(--muted-foreground))" }}>
                  Tail
                </span>
              )}
              <button
                onClick={refreshDebugLogs}
                disabled={debugBusy}
                className="w-8 h-8 rounded-lg flex items-center justify-center transition-colors disabled:opacity-50"
                style={{ background: "hsl(var(--surface-4))", color: "hsl(var(--muted-foreground))" }}
                title="Refresh logs"
              >
                <RefreshCw size={13} className={debugBusy ? "animate-spin" : ""} />
              </button>
            </div>
          </div>

          <div className="panel overflow-hidden">
            <div className="px-5 pt-4 pb-3 flex items-start gap-3">
              <div
                className="w-9 h-9 rounded-xl flex items-center justify-center flex-shrink-0 text-muted-foreground"
                style={{ background: "hsl(var(--surface-4))" }}
              >
                <Bug size={16} />
              </div>
              <div className="min-w-0 flex-1">
                <p className="text-[13px] font-medium text-foreground">Latest run</p>
                <p className="text-[11px] text-muted-foreground mt-1 truncate">
                  {debugTab === "backend"
                    ? debugLogs?.backend_path ?? "backend.log"
                    : debugTab === "desktop"
                    ? debugLogs?.desktop_path ?? "said.log"
                    : `${debugLogs?.desktop_path ?? "said.log"} + ${debugLogs?.backend_path ?? "backend.log"}`}
                </p>
              </div>
            </div>

            <div className="mx-5 border-t" style={{ borderColor: "hsl(var(--surface-3))" }} />

            <div className="px-5 py-3 flex items-center justify-between gap-3">
              <div
                className="flex rounded-xl p-0.5 gap-0.5"
                style={{ background: "hsl(var(--surface-4))" }}
              >
                {([
                  ["combined", "Combined"],
                  ["desktop",  "Said"],
                  ["backend",  "Backend"],
                ] as const).map(([id, label]) => {
                  const active = debugTab === id;
                  return (
                    <button
                      key={id}
                      onClick={() => setDebugTab(id)}
                      className="text-[12px] font-medium rounded-[10px] px-3 py-1.5 transition-all"
                      style={{
                        background: active ? "hsl(var(--surface-1))" : "transparent",
                        color: active ? "hsl(var(--foreground))" : "hsl(var(--muted-foreground))",
                      }}
                    >
                      {label}
                    </button>
                  );
                })}
              </div>

              <button
                onClick={() => copyDebugLog(debugTab)}
                disabled={!debugLogs || !(debugLogs[debugTab] ?? "").trim()}
                className="text-[12px] font-semibold px-3 py-1.5 rounded-lg flex items-center gap-1.5 transition-colors disabled:opacity-50"
                style={{ background: "hsl(var(--surface-4))", color: "hsl(var(--muted-foreground))" }}
              >
                {debugCopied === debugTab ? <Check size={12} /> : <Copy size={12} />}
                {debugCopied === debugTab ? "Copied" : "Copy"}
              </button>
            </div>

            <div className="px-5 pb-5">
              <div
                className="rounded-xl overflow-hidden border"
                style={{ borderColor: "hsl(var(--surface-4))", background: "hsl(var(--surface-1))" }}
              >
                <div
                  className="flex items-center gap-2 px-3 py-2 border-b"
                  style={{ borderColor: "hsl(var(--surface-4))", color: "hsl(var(--muted-foreground))" }}
                >
                  <FileText size={12} />
                  <span className="text-[11px] font-medium">
                    {debugBusy ? "Loading" : debugTab === "combined" ? "Combined" : debugTab === "desktop" ? "Said desktop" : "polish-backend"}
                  </span>
                </div>
                <textarea
                  readOnly
                  value={
                    debugBusy
                      ? "Loading logs..."
                      : debugLogs
                      ? debugLogs[debugTab] || "(empty)"
                      : "(logs unavailable)"
                  }
                  spellCheck={false}
                  className="w-full h-[340px] resize-none bg-transparent px-3 py-3 font-mono text-[11px] leading-relaxed outline-none"
                  style={{ color: "hsl(var(--foreground))" }}
                />
              </div>
            </div>
          </div>
        </div>
        </Show>

        {/* ── About ────────────────────────────────────── */}
        <Show when={isOn("about")}>
        <Section title="About">
          <Row
            icon={<Info size={16} />}
            label="Said — Voice Polish Studio"
            description="Local-first · Built with Tauri + Rust + React"
          />
          <Row
            icon={<Download size={16} />}
            label="Software Update"
            description={
              updateStatus === "checking" ? "Checking for updates…" :
              updateStatus === "available" ? `Version ${updateVersion} is available` :
              updateStatus === "downloading" ? "Downloading update…" :
              updateStatus === "ready" ? "Update installed — relaunch to finish" :
              updateStatus === "up-to-date" ? "You're on the latest version" :
              updateStatus === "error" ? (updateError || "Update check failed") :
              "Check for available updates"
            }
            last
            action={
              <div className="flex items-center gap-2">
                {updateStatus === "checking" || updateStatus === "downloading" ? (
                  <Loader2 size={14} className="animate-spin text-muted-foreground" />
                ) : updateStatus === "available" ? (
                  <button
                    onClick={() => void downloadAndInstall()}
                    className="px-3 py-1 rounded-md text-[11px] font-medium border border-transparent text-background"
                    style={{ background: "hsl(var(--primary))" }}
                  >
                    Install Update
                  </button>
                ) : updateStatus === "ready" ? (
                  <button
                    onClick={() => void relaunch()}
                    className="px-3 py-1 rounded-md text-[11px] font-medium border border-transparent text-background"
                    style={{ background: "hsl(var(--primary))" }}
                  >
                    Relaunch
                  </button>
                ) : (
                  <button
                    onClick={() => void checkForUpdates()}
                    className="px-3 py-1 rounded-md text-[11px] font-medium border border-border text-muted-foreground hover:text-foreground hover:border-foreground/30 transition-all"
                  >
                    Check
                  </button>
                )}
              </div>
            }
          />
        </Section>
        </Show>

    </>
  );

  if (embedded) return inner;
  return (
    <ScrollArea className="h-full">
      <div className="p-6 pb-10 max-w-2xl mx-auto">
        {inner}
      </div>
    </ScrollArea>
  );
}
