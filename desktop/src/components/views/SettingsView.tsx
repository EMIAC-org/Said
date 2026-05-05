import React, { useEffect, useRef, useState } from "react";
import { ScrollArea } from "@/components/ui/scroll-area";
import { cn } from "@/lib/utils";
import {
  Shield, Cpu, Key, Info, Wifi, Check, Bot, Sparkles, Zap,
  Languages, MessageSquareText, Loader2, Cloud, LogIn, LogOut, RefreshCw, UserPlus,
  Eye, EyeOff, Bell, Bug, Copy, FileText, Mic, MonitorUp,
} from "lucide-react";
import type { AppSnapshot, CloudStatus, OpenAIStatus, Preferences } from "@/types";
import {
  cloudLogin, cloudLogout, cloudSignup, getCloudStatus,
  getPreferences, patchPreferences,
  getOpenAIStatus, initiateOpenAIOAuth, disconnectOpenAI,
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
  | "account"
  | "debug"
  | "about";

export const SETTINGS_SECTIONS: { id: SettingsSection; label: string }[] = [
  { id: "writing",     label: "Writing style" },
  { id: "permissions", label: "Permissions"   },
  { id: "api-keys",    label: "API keys"      },
  { id: "account",     label: "Account"       },
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
  onScreenRecording: () => void;
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
  onScreenRecording,
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
  const screenGranted = snapshot?.screen_recording_granted ?? false;

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

  // ── Cloud auth state ─────────────────────────────────────────────────────────
  // ── OpenAI OAuth state ──────────────────────────────────────────────────────
  const [openAIStatus,  setOpenAIStatus]  = useState<OpenAIStatus | null>(null);
  const [openAIBusy,    setOpenAIBusy]    = useState(false);
  const [openAIError,   setOpenAIError]   = useState("");

  const [cloudStatus,  setCloudStatus]  = useState<CloudStatus | null>(null);
  const [cloudEmail,   setCloudEmail]   = useState("");
  const [cloudPass,    setCloudPass]    = useState("");
  const [cloudMode,    setCloudMode]    = useState<"login" | "signup">("login");
  const [cloudBusy,    setCloudBusy]    = useState(false);
  const [cloudError,   setCloudError]   = useState("");

  // ── Debug logs state ───────────────────────────────────────────────────────
  const [debugLogs,    setDebugLogs]    = useState<DebugLogs | null>(null);
  const [debugBusy,    setDebugBusy]    = useState(false);
  const [debugCopied,  setDebugCopied]  = useState<"combined" | "desktop" | "backend" | null>(null);
  const [debugTab,     setDebugTab]     = useState<"combined" | "desktop" | "backend">("combined");
  const recordHotkey = prefs?.record_hotkey ?? "caps_lock";
  const recordHotkeyLabel =
    recordHotkey === "right_option" ? "Right Option" :
    recordHotkey === "fn" ? "Fn" :
    "Caps Lock";

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
        // Prefill key fields with masked placeholders only if keys already stored
        // (don't reveal the actual key — user can re-enter to change)
        setGatewayKey(p.gateway_api_key ? "••••••••••••••••" : "");
        setDeepgramKey(p.deepgram_api_key ? "••••••••••••••••" : "");
        setGeminiKey(p.gemini_api_key ? "••••••••••••••••" : "");
        setGroqKey(p.groq_api_key ? "••••••••••••••••" : "");
      }
    });
    getCloudStatus().then((s) => { if (s) setCloudStatus(s); });
    getOpenAIStatus().then((s) => { if (s) setOpenAIStatus(s); });
  }, []);

  async function handleCloudAuth() {
    setCloudBusy(true);
    setCloudError("");
    try {
      const resp = cloudMode === "login"
        ? await cloudLogin(cloudEmail, cloudPass)
        : await cloudSignup(cloudEmail, cloudPass);
      setCloudStatus({
        connected:    true,
        license_tier: resp.account.license_tier,
        email:        resp.account.email,
      });
      setCloudEmail("");
      setCloudPass("");
    } catch (err) {
      setCloudError(err instanceof Error ? err.message : String(err));
    } finally {
      setCloudBusy(false);
    }
  }

  async function handleCloudLogout() {
    setCloudBusy(true);
    try {
      await cloudLogout();
      setCloudStatus({ connected: false, license_tier: "free", email: null });
    } finally {
      setCloudBusy(false);
    }
  }

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

  async function handleOpenAIConnect() {
    setOpenAIBusy(true);
    setOpenAIError("");
    try {
      await initiateOpenAIOAuth();
      // Poll every 2s for up to 5 min until the backend confirms connection
      let attempts = 0;
      const poll = setInterval(async () => {
        attempts++;
        const s = await getOpenAIStatus();
        if (s?.connected) {
          setOpenAIStatus(s);
          setPrefs((prev) => prev ? { ...prev, llm_provider: "openai_codex" } : prev);
          setOpenAIBusy(false);
          clearInterval(poll);
        } else if (attempts > 150) {
          setOpenAIError("Timed out. Please try again.");
          setOpenAIBusy(false);
          clearInterval(poll);
        }
      }, 2000);
    } catch (err) {
      setOpenAIError(err instanceof Error ? err.message : String(err));
      setOpenAIBusy(false);
    }
  }

  async function handleOpenAIDisconnect() {
    setOpenAIBusy(true);
    try {
      await disconnectOpenAI();
      setOpenAIStatus((prev) => prev ? { ...prev, connected: false } : null);
      setPrefs((prev) => prev ? { ...prev, llm_provider: "gateway" } : prev);
    } catch (err) {
      setOpenAIError(err instanceof Error ? err.message : String(err));
    } finally {
      setOpenAIBusy(false);
    }
  }

  async function saveApiKeys() {
    setKeySaving(true);
    setSaveError("");
    try {
      const update: Partial<Preferences> = {};
      // Only send if the user has entered a real key (not the placeholder bullets)
      if (gatewayKey  && !gatewayKey.startsWith("••"))   update.gateway_api_key  = gatewayKey;
      if (deepgramKey && !deepgramKey.startsWith("••"))  update.deepgram_api_key = deepgramKey;
      if (geminiKey   && !geminiKey.startsWith("••"))    update.gemini_api_key   = geminiKey;
      if (groqKey     && !groqKey.startsWith("••"))      update.groq_api_key     = groqKey;
      const updated = await patchPreferences(update);
      if (updated) {
        setPrefs(updated);
        // Re-mask the inputs to bullets so the UI clearly reflects "saved"
        // (otherwise the user keeps seeing the raw key they just typed and
        // wonders whether it persisted)
        const MASK = "••••••••••••••••";
        if (updated.gateway_api_key)  setGatewayKey(MASK);
        if (updated.deepgram_api_key) setDeepgramKey(MASK);
        if (updated.gemini_api_key)   setGeminiKey(MASK);
        if (updated.groq_api_key)     setGroqKey(MASK);
        // Hide reveal-toggles after save so bullets aren't accidentally exposed
        setShowGateway(false);
        setShowDeepgram(false);
        setShowGemini(false);
        setShowGroq(false);
      }
      // Show brief "Saved ✓" feedback
      setKeySaved(true);
      if (keySaveTimer.current) clearTimeout(keySaveTimer.current);
      keySaveTimer.current = setTimeout(() => setKeySaved(false), 2500);
    } catch {
      setSaveError("Failed to save — is the backend running?");
    } finally {
      setKeySaving(false);
    }
  }

  async function patch(update: Partial<Preferences>) {
    if (!prefs) return;
    console.log("[patch] calling patchPreferences with:", JSON.stringify(update));
    setSaving(true);
    setSaveError("");
    try {
      const updated = await patchPreferences(update);
      console.log("[patch] got back:", updated ? JSON.stringify({ llm_provider: updated.llm_provider }) : "null");
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

            <div className="mx-5 border-t" style={{ borderColor: "hsl(var(--surface-3))" }} />

            {/* Row 5: Screen Recording */}
            <div className="flex items-center gap-4 px-5 py-4">
              <div
                className="w-9 h-9 rounded-xl flex items-center justify-center flex-shrink-0"
                style={{ background: "hsl(var(--surface-4))", color: "hsl(var(--muted-foreground))" }}
              >
                <MonitorUp size={16} />
              </div>
              <div className="flex-1 min-w-0">
                <p className="text-[13px] font-medium text-foreground">Screen Recording</p>
                <p className="text-[12px] text-muted-foreground mt-0.5 leading-relaxed">
                  {screenGranted
                    ? "Granted — optional context awareness is available."
                    : "Optional for future context awareness. Dictation works without it."}
                </p>
              </div>
              <div className="flex-shrink-0 ml-4">
                {axSupported ? (
                  screenGranted ? (
                    <span
                      className="text-[12px] font-semibold px-3 py-1.5 rounded-lg flex items-center gap-1"
                      style={{ background: "hsl(var(--surface-4))", color: "hsl(var(--muted-foreground))" }}
                    >
                      <Check size={11} /> Granted
                    </span>
                  ) : (
                    <button
                      onClick={onScreenRecording}
                      className="text-[12px] font-semibold px-3 py-1.5 rounded-lg transition-colors"
                      style={{ background: "hsl(var(--primary))", color: "hsl(var(--primary-foreground))" }}
                    >
                      Allow
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
              Keys are stored locally in SQLite — they never leave your Mac.
              Enter each key once; you only need to re-enter to change it.
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
                  onFocus={() => {
                    if (gatewayKey.startsWith("••")) setGatewayKey("");
                  }}
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
                  onFocus={() => {
                    if (deepgramKey.startsWith("••")) setDeepgramKey("");
                  }}
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
              <p className="text-[12px] font-semibold text-foreground mb-1.5 flex items-center gap-1.5">
                <Sparkles size={12} className="text-muted-foreground" />
                Gemini API Key
              </p>
              <div className="relative">
                <input
                  type={showGemini ? "text" : "password"}
                  placeholder="AIza…"
                  value={geminiKey}
                  onChange={(e) => setGeminiKey(e.target.value)}
                  onFocus={() => {
                    if (geminiKey.startsWith("••")) setGeminiKey("");
                  }}
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
                  onFocus={() => {
                    if (groqKey.startsWith("••")) setGroqKey("");
                  }}
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
              id:       "gateway",
              icon:     <Wifi size={15} />,
              label:    "Gateway",
              desc:     "gpt-5.4-mini via gateway.voicepolish.app — no key needed",
              badge:    "Default",
              needsKey: false,
            },
            {
              id:    "groq",
              icon:  <Zap size={15} />,
              label: "Groq LPU",
              desc:  "llama-3.3-70b-versatile — fastest (~200ms TTFT), free tier",
              badge: "Fast",
              // Key is present if: already saved in prefs OR user has typed one in the input
              needsKey: !prefs?.groq_api_key && !(groqKey && !groqKey.startsWith("••")),
            },
            {
              id:    "gemini_direct",
              icon:  <Sparkles size={15} />,
              label: "Gemini Direct",
              desc:  "gemini-2.0-flash-thinking via Google AI — needs Gemini API key",
              badge: null,
              needsKey: !prefs?.gemini_api_key && !(geminiKey && !geminiKey.startsWith("••")),
            },
            {
              id:    "openai_codex",
              icon:  <Bot size={15} />,
              label: "OpenAI Codex",
              desc:  "gpt-5.4-mini via ChatGPT OAuth — connect account below",
              badge: null,
              needsKey: !openAIStatus?.connected,
            },
          ] as const).map((opt, idx, arr) => {
            const isOpenAIDisconnected = opt.id === "openai_codex" && !openAIStatus?.connected;
            const isActive = prefs?.llm_provider === opt.id && !isOpenAIDisconnected;
            const showReconnect = opt.id === "openai_codex" && isOpenAIDisconnected;
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
                      onClick={() => {
                        if (showReconnect) {
                          void handleOpenAIConnect();
                          return;
                        }
                        void patch({ llm_provider: opt.id });
                      }}
                      className={`px-3 py-1 rounded-md text-[11px] font-medium transition-all border ${
                        isActive
                          ? "border-transparent text-background"
                          : "border-border text-muted-foreground hover:text-foreground hover:border-foreground/30"
                      }`}
                      style={isActive ? { background: "hsl(var(--muted-foreground))" } : {}}
                    >
                      {showReconnect ? "Reconnect" : isActive ? "✓ Active" : "Use"}
                    </button>
                  </div>
                }
              />
            );
          })}
        </Section>
        </Show>

        {/* ── OpenAI Account ────────────────────────────── */}
        <Show when={isOn("account")}>
        <div className="mb-7">
          <p className="section-label px-1 mb-2.5">OpenAI Account</p>

          {openAIStatus?.connected ? (
            /* ── Connected state ──────────────────────── */
            <div className="panel p-5 space-y-4">
              {/* Status row */}
              <div className="flex items-center justify-between">
                <div className="flex items-center gap-2.5">
                  <div
                    className="w-9 h-9 rounded-xl flex items-center justify-center flex-shrink-0"
                    style={{ background: "hsl(var(--surface-4))" }}
                  >
                    <Check size={16} style={{ color: "hsl(var(--muted-foreground))" }} />
                  </div>
                  <div>
                    <p className="text-[13px] font-semibold text-foreground leading-tight">ChatGPT Connected</p>
                    <p className="text-[11px] text-muted-foreground">OAuth token stored locally · models ready</p>
                  </div>
                </div>
                <div className="flex items-center gap-3">
                  <button
                    onClick={handleOpenAIConnect}
                    disabled={openAIBusy}
                    className="flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground transition-colors"
                  >
                    <RefreshCw size={12} />
                    {openAIBusy ? "…" : "Reconnect"}
                  </button>
                  <button
                    onClick={handleOpenAIDisconnect}
                    disabled={openAIBusy}
                    className="flex items-center gap-1 text-xs text-muted-foreground hover:text-red-500 transition-colors"
                  >
                    <LogOut size={12} />
                    Disconnect
                  </button>
                </div>
              </div>

              {openAIBusy && (
                <p className="text-[11px] text-muted-foreground">Waiting for browser sign-in… this window updates automatically.</p>
              )}
              {openAIError && (
                <p className="text-[11px]" style={{ color: "hsl(0 75% 75%)" }}>{openAIError}</p>
              )}
            </div>
          ) : (
            /* ── Not connected state ──────────────────── */
            <div className="panel p-5 space-y-4">
              <div className="flex items-start gap-3">
                <div
                  className="w-9 h-9 rounded-xl flex items-center justify-center flex-shrink-0 mt-0.5"
                  style={{ background: "hsl(var(--surface-4))" }}
                >
                  <Sparkles size={16} className="text-muted-foreground" />
                </div>
                <div>
                  <p className="text-[13px] font-semibold text-foreground leading-tight">Connect your ChatGPT account</p>
                  <p className="text-[12px] text-muted-foreground mt-1 leading-relaxed">
                    Use your ChatGPT Pro subscription to access <strong className="text-foreground">gpt-5.4</strong> and{" "}
                    <strong className="text-foreground">gpt-5.4-mini</strong> directly — no API key needed.
                    Your voice data never leaves your Mac.
                  </p>
                </div>
              </div>

              <button
                onClick={handleOpenAIConnect}
                disabled={openAIBusy}
                className="w-full btn-primary flex items-center justify-center gap-2 !py-2.5"
              >
                {openAIBusy ? (
                  <>
                    <Loader2 size={13} className="animate-spin" />
                    Waiting for browser…
                  </>
                ) : (
                  <>
                    <LogIn size={13} />
                    Connect OpenAI account
                  </>
                )}
              </button>

              {openAIBusy && (
                <p className="text-[11px] text-muted-foreground text-center leading-relaxed">
                  A browser window will open. Sign in with your ChatGPT account, then return here.
                </p>
              )}

              {openAIError && (
                <p className="text-[11px]" style={{ color: "hsl(0 75% 75%)" }}>{openAIError}</p>
              )}
            </div>
          )}
        </div>

        {/* ── Cloud Account ─────────────────────────────── */}
        <Section title="Cloud Account">
          {cloudStatus?.connected ? (
            /* ── Connected state ───────────────────────── */
            <Row
              icon={<Cloud size={16} />}
              label={cloudStatus.email ?? "Connected"}
              description={`License: ${cloudStatus.license_tier} tier`}
              action={
                <button
                  onClick={handleCloudLogout}
                  disabled={cloudBusy}
                  className="flex items-center gap-1 text-xs text-muted-foreground hover:text-red-600 transition-colors"
                >
                  <LogOut size={12} />
                  {cloudBusy ? "…" : "Sign out"}
                </button>
              }
              last
            />
          ) : (
            /* ── Auth form ─────────────────────────────── */
            <div className="px-5 py-4">
              <div className="flex items-center gap-3 mb-3">
                <div
                  className="w-9 h-9 rounded-xl flex items-center justify-center flex-shrink-0 text-muted-foreground"
                  style={{ background: "hsl(var(--surface-4))" }}
                >
                  <Cloud size={16} />
                </div>
                <div>
                  <p className="text-[13px] font-medium text-foreground">Sign in to Said Cloud</p>
                  <p className="text-[12px] text-muted-foreground mt-0.5">
                    Enables usage metering and license validation
                  </p>
                </div>
              </div>

              {/* Mode toggle */}
              <div className="flex gap-2 mb-3">
                {(["login", "signup"] as const).map((m) => (
                  <button
                    key={m}
                    onClick={() => { setCloudMode(m); setCloudError(""); }}
                    className={cn("pill", cloudMode === m && "active")}
                  >
                    {m === "login" ? <LogIn size={11} /> : <UserPlus size={11} />}
                    {m === "login" ? "Sign in" : "Create account"}
                  </button>
                ))}
              </div>

              {/* Inputs */}
              <div className="space-y-2 mb-3">
                <input
                  type="email"
                  placeholder="Email"
                  value={cloudEmail}
                  onChange={(e) => setCloudEmail(e.target.value)}
                  className="input"
                />
                <input
                  type="password"
                  placeholder="Password"
                  value={cloudPass}
                  onChange={(e) => setCloudPass(e.target.value)}
                  onKeyDown={(e) => { if (e.key === "Enter") handleCloudAuth(); }}
                  className="input"
                />
              </div>

              {cloudError && (
                <p className="text-[12px] mb-2" style={{ color: "hsl(0 75% 75%)" }}>
                  {cloudError}
                </p>
              )}

              <button
                onClick={handleCloudAuth}
                disabled={cloudBusy || !cloudEmail || !cloudPass}
                className="btn-primary w-full justify-center"
              >
                {cloudBusy ? (
                  <Loader2 size={13} className="inline animate-spin mr-1" />
                ) : null}
                {cloudMode === "login" ? "Sign in" : "Create account"}
              </button>
            </div>
          )}
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
            description="Version 0.1.0 · Local-first · Built with Tauri + Rust + React"
            last
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
