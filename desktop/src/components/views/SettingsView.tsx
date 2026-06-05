import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ScrollArea } from "@/components/ui/scroll-area";
import { cn } from "@/lib/utils";
import { getVersion } from "@tauri-apps/api/app";
import {
  Shield, Cpu, Key, Info, Wifi, Check, Sparkles, Zap,
  Languages, MessageSquareText, Loader2, RefreshCw,
  Eye, EyeOff, Bell, Bug, Copy, FileText, Mic, Download, Activity,
  RotateCcw, Save, GitCompareArrows, Play, Link, LogOut, ChevronDown,
} from "lucide-react";
import { check } from "@tauri-apps/plugin-updater";
import { applyPendingUpdate, downloadUpdate } from "@/lib/autoUpdate";
import type { AppSnapshot, Preferences, PromptTemplateResponse, PromptTestResponse } from "@/types";
import { AppearanceSection } from "@/components/views/AppearanceSection";

import {
  getConnection as enterpriseGetConnection,
  disconnectEnterprise,
  type EnterpriseConnection,
} from "@/lib/enterprise";
import { EnterpriseConnectForm } from "@/components/EnterpriseConnectForm";
import {
  getPreferences, patchPreferences,
  getVoicePrompt, saveVoicePromptDraft, applyVoicePromptDraft,
  resetVoicePrompt, testVoicePrompt,
  getDebugLogs,
  requestNotifications, checkNotificationPermission,
  getDesktopPrefs, setDesktopPrefs,
  readBackendLog, backendLogLocation, openLogFolder,
  openaiConnect, openaiStatus, openaiDisconnect,
  type DebugLogs,
  type NotifPermission,
  type DesktopPrefs,
  type OpenAIStatus,
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

function SecretInput({
  icon,
  label,
  helper,
  placeholder,
  value,
  visible,
  onChange,
  onToggle,
}: {
  icon:        React.ReactNode;
  label:       string;
  helper?:     string;
  placeholder: string;
  value:       string;
  visible:     boolean;
  onChange:    (value: string) => void;
  onToggle:    () => void;
}) {
  return (
    <div>
      <div className="mb-1.5">
        <p className="text-[12px] font-semibold text-foreground flex items-center gap-1.5">
          {icon}
          {label}
        </p>
        {helper && (
          <p className="text-[11px] text-muted-foreground mt-0.5">
            {helper}
          </p>
        )}
      </div>
      <div className="relative">
        <input
          type={visible ? "text" : "password"}
          placeholder={placeholder}
          value={value}
          onChange={(e) => onChange(e.target.value)}
          className="input pr-9 font-mono text-[12px]"
        />
        <button
          type="button"
          onClick={onToggle}
          className="absolute right-2.5 top-1/2 -translate-y-1/2 text-muted-foreground hover:text-foreground transition-colors"
          tabIndex={-1}
          aria-label={visible ? `Hide ${label}` : `Show ${label}`}
        >
          {visible ? <EyeOff size={14} /> : <Eye size={14} />}
        </button>
      </div>
    </div>
  );
}

function SettingsDisclosure({
  title,
  description,
  icon,
  defaultOpen = false,
  status,
  children,
}: {
  title:        string;
  description:  string;
  icon:         React.ReactNode;
  defaultOpen?: boolean;
  status?:      React.ReactNode;
  children:     React.ReactNode;
}) {
  const [open, setOpen] = useState(defaultOpen);

  return (
    <div
      className="rounded-xl overflow-hidden"
      style={{
        background: "hsl(var(--surface-3))",
        boxShadow: "inset 0 0 0 1px hsl(var(--surface-4))",
      }}
    >
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="w-full flex items-center gap-3 px-4 py-3 text-left"
        aria-expanded={open}
      >
        <span
          className="w-8 h-8 rounded-lg flex items-center justify-center flex-shrink-0"
          style={{
            background: "hsl(var(--surface-4))",
            color: "hsl(var(--accent-violet))",
          }}
        >
          {icon}
        </span>
        <span className="min-w-0 flex-1">
          <span className="block text-[13px] font-semibold text-foreground">
            {title}
          </span>
          <span className="block text-[11.5px] text-muted-foreground mt-0.5 leading-relaxed">
            {description}
          </span>
        </span>
        {status && <span className="flex-shrink-0">{status}</span>}
        <ChevronDown
          size={15}
          className="flex-shrink-0 text-muted-foreground transition-transform"
          style={{ transform: open ? "rotate(180deg)" : "rotate(0deg)" }}
        />
      </button>
      {open && (
        <div
          className="px-4 pb-4 pt-1 space-y-4"
          style={{ borderTop: "1px solid hsl(var(--surface-4))" }}
        >
          {children}
        </div>
      )}
    </div>
  );
}

// ── ChatGPT Connection Section ────────────────────────────────────────────────

function ChatGPTSection() {
  const [status, setStatus] = useState<OpenAIStatus | null>(null);
  const [connecting, setConnecting] = useState(false);

  const refresh = useCallback(async () => {
    const s = await openaiStatus();
    setStatus(s);
  }, []);

  useEffect(() => { void refresh(); }, [refresh]);

  // Poll for connection after initiating OAuth (browser flow)
  useEffect(() => {
    if (!connecting) return;
    const interval = setInterval(async () => {
      const s = await openaiStatus();
      if (s?.connected) {
        setStatus(s);
        setConnecting(false);
      }
    }, 2000);
    return () => clearInterval(interval);
  }, [connecting]);

  const handleConnect = async () => {
    setConnecting(true);
    await openaiConnect();
  };

  const handleDisconnect = async () => {
    await openaiDisconnect();
    setStatus({ connected: false, expires_at: null, connected_at: null });
  };

  const isConnected = status?.connected === true;
  const expiresDate = status?.expires_at
    ? new Date(status.expires_at).toLocaleDateString("en-IN", { day: "numeric", month: "short", year: "numeric" })
    : null;

  return (
    <Section title="ChatGPT">
      <Row
        icon={<Sparkles size={16} />}
        label={isConnected ? "ChatGPT Connected" : "Connect ChatGPT"}
        description={
          isConnected
            ? `Used for shortcut transforms and repair/refine${expiresDate ? ` · expires ${expiresDate}` : ""}`
            : connecting
            ? "Waiting for browser sign-in..."
            : "Optional. AirNote falls back to Groq when ChatGPT is not connected."
        }
        action={
          <div className="flex items-center gap-2">
            {isConnected && (
              <span className="text-[10px] px-1.5 py-0.5 rounded"
                    style={{ background: "hsl(145 60% 15%)", color: "hsl(145 70% 65%)" }}>
                Connected
              </span>
            )}
            {connecting && (
              <Loader2 size={14} className="animate-spin" style={{ color: "hsl(var(--muted-foreground))" }} />
            )}
            {isConnected ? (
              <button
                onClick={() => void handleDisconnect()}
                className="px-3 py-1 rounded-md text-[11px] font-medium transition-all border border-border text-muted-foreground hover:text-foreground hover:border-foreground/30"
              >
                Disconnect
              </button>
            ) : (
              <button
                onClick={() => void handleConnect()}
                disabled={connecting}
                className="px-3 py-1 rounded-md text-[11px] font-medium transition-all border border-transparent"
                style={{ background: "hsl(var(--accent-violet))", color: "#fff", opacity: connecting ? 0.6 : 1 }}
              >
                {connecting ? "Connecting..." : "Connect"}
              </button>
            )}
          </div>
        }
      />
    </Section>
  );
}

// ── Section routing (used by SettingsModal) ───────────────────────────────────

export type SettingsSection =
  | "appearance"
  | "writing"
  | "models"
  | "notifications"
  | "permissions"
  | "api-keys"
  | "enterprise"
  | "debug"
  | "about";

export const SETTINGS_SECTIONS: { id: SettingsSection; label: string }[] = [
  { id: "models",         label: "Models"         },
  { id: "notifications",  label: "Notifications"  },
  { id: "permissions",    label: "Permissions"     },
  { id: "api-keys",    label: "API keys"      },
  { id: "enterprise",  label: "Enterprise"    },
  { id: "about",       label: "About"         },
];

function Show({ when, children }: { when: boolean; children: React.ReactNode }) {
  return when ? <>{children}</> : null;
}

type DiffLine = {
  type: "same" | "add" | "remove";
  text: string;
};

type PromptPreviewMode = "rendered" | "diff";

function splitPromptLines(value: string): string[] {
  return value.replace(/\r\n/g, "\n").split("\n");
}

function diffPromptLines(before: string, after: string): DiffLine[] {
  const a = splitPromptLines(before);
  const b = splitPromptLines(after);
  const dp = Array.from({ length: a.length + 1 }, () =>
    Array<number>(b.length + 1).fill(0)
  );

  for (let i = a.length - 1; i >= 0; i -= 1) {
    for (let j = b.length - 1; j >= 0; j -= 1) {
      dp[i][j] = a[i] === b[j]
        ? dp[i + 1][j + 1] + 1
        : Math.max(dp[i + 1][j], dp[i][j + 1]);
    }
  }

  const out: DiffLine[] = [];
  let i = 0;
  let j = 0;
  while (i < a.length && j < b.length) {
    if (a[i] === b[j]) {
      out.push({ type: "same", text: a[i] });
      i += 1;
      j += 1;
    } else if (dp[i + 1][j] >= dp[i][j + 1]) {
      out.push({ type: "remove", text: a[i] });
      i += 1;
    } else {
      out.push({ type: "add", text: b[j] });
      j += 1;
    }
  }
  while (i < a.length) {
    out.push({ type: "remove", text: a[i] });
    i += 1;
  }
  while (j < b.length) {
    out.push({ type: "add", text: b[j] });
    j += 1;
  }

  return out;
}

function formatPromptTime(ms: number | null): string {
  if (!ms) return "Never";
  return new Date(ms).toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function languageRulePreview(outputLanguage: string): string {
  if (outputLanguage === "english") {
    return [
      "- Output language: English.",
      "- Write natural English only. Translate non-English words when needed.",
    ].join("\n");
  }
  if (outputLanguage === "hindi") {
    return [
      "- Output language: Hindi.",
      "- Write natural Hindi in Devanagari script.",
    ].join("\n");
  }
  return [
    "- Output language: Roman Hinglish.",
    "- Detect the language of each span in the transcript independently.",
    "- English spans stay English.",
    '- Hindi spans, including Devanagari input, become Roman Hinglish; transliterate to Roman script, e.g. "यह" -> "Yeh". NEVER output Devanagari. NEVER translate Hindi to English.',
    "- Hinglish spans stay Hinglish Roman.",
    "- Do NOT make the whole output uniform. Preserve the speaker's mix.",
    "",
    "Examples:",
    'Input: "Bahut sahi baat hai yaar. How much time will it take to go ahead?"',
    'Output: "Bahut sahi baat hai yaar. How much time will it take to go ahead?"',
    'Input: "यह बहुत सही बात है yaar. Please check this tomorrow."',
    'Output: "Yeh bahut sahi baat hai yaar. Please check this tomorrow."',
  ].join("\n");
}

function personaPreview(tonePreset: ToneKey, customPrompt: string): string {
  const trimmed = customPrompt.trim();
  if (tonePreset === "custom" && trimmed) {
    return [
      "STYLE NOTE (advisory tone hint only — does not override the cleaning rules above):",
      trimmed.slice(0, 500),
    ].join("\n");
  }
  return "Be faithful to the spoken words first, then make them clear.";
}

function toneDescriptionPreview(tonePreset: ToneKey): string {
  switch (tonePreset) {
    case "professional":
      return "Tone: formal and professional. Polish wording for work communication while preserving the speaker's request intent, politeness markers, and content words. Do not delete words like please, kindly, thanks, can you, could you, would you, just, once, or zara.";
    case "casual":
      return "Tone: friendly and conversational. Light and easy to read. Preserve the speaker's human tone and request-softening words.";
    case "assertive":
      return "Tone: direct and confident. Clear calls-to-action, but do not turn polite requests into blunt commands or delete request-softening words.";
    case "concise":
      return "Tone: minimal words. Remove filler noise and redundant phrasing only; preserve all meaningful words, politeness markers, and request softeners.";
    case "custom":
      return "Tone: neutral and clear.";
    case "neutral":
    default:
      return "Tone: neutral and clear. No strong stylistic lean.";
  }
}

function replaceTemplateToken(template: string, token: string, value: string): string {
  return template.split(token).join(value);
}

function renderPromptTemplatePreview(
  template: string,
  tonePreset: ToneKey,
  customPrompt: string,
  outputLanguage: string
): string {
  return [
    ["{{language_rule}}", languageRulePreview(outputLanguage)],
    ["{{persona}}", personaPreview(tonePreset, customPrompt)],
    ["{{tone}}", toneDescriptionPreview(tonePreset)],
    ["{{vocab_block}}", ""],
    ["{{corrections_block}}", ""],
    ["{{prefs_block}}", ""],
  ].reduce(
    (next, [token, value]) => replaceTemplateToken(next, token, value),
    template
  );
}

// ── Enterprise section ────────────────────────────────────────────────────────

function EnterpriseSection({ onDisconnect }: { onDisconnect?: () => void }) {
  const [connection, setConnection] = useState<EnterpriseConnection | null>(null);

  useEffect(() => {
    setConnection(enterpriseGetConnection());
  }, []);

  async function handleDisconnect() {
    await disconnectEnterprise();
    setConnection(null);
    onDisconnect?.();
  }

  if (connection) {
    return (
      <div className="mb-7">
        <p className="section-label px-1 mb-2.5 flex items-center gap-2">
          <span
            className="inline-block w-1 h-1 rounded-full"
            style={{ background: "hsl(var(--accent-violet))" }}
          />
          Enterprise
        </p>
        <div className="panel overflow-hidden">
          <div className="flex items-center gap-4 px-5 py-4">
            <div
              className="w-9 h-9 rounded-xl flex items-center justify-center flex-shrink-0 overflow-hidden"
              style={{
                background: "hsl(var(--surface-4))",
                color: "hsl(var(--accent-violet))",
              }}
            >
              {connection.larkAvatarUrl ? (
                <img
                  src={connection.larkAvatarUrl}
                  alt=""
                  className="w-full h-full object-cover"
                />
              ) : (
                <Link size={16} />
              )}
            </div>
            <div className="flex-1 min-w-0">
              <div className="flex items-center gap-2">
                <p className="text-[13px] font-medium text-foreground">
                  {connection.orgName ?? "Enterprise"}
                </p>
                <span
                  className="text-[10px] font-semibold px-2 py-0.5 rounded-full"
                  style={{
                    background: "hsl(145 60% 16%)",
                    color: "hsl(145 70% 65%)",
                  }}
                >
                  Connected
                </span>
              </div>
              <p className="text-[12px] text-muted-foreground mt-0.5 leading-relaxed truncate">
                {connection.larkName
                  ? `${connection.larkName} · ${connection.email}`
                  : connection.email}
              </p>
            </div>
          </div>

          <div className="mx-5 border-t" style={{ borderColor: "hsl(var(--surface-3))" }} />

          <div className="flex items-center gap-4 px-5 py-4">
            <div
              className="w-9 h-9 rounded-xl flex items-center justify-center flex-shrink-0"
              style={{
                background: "hsl(var(--surface-4))",
                color: "hsl(var(--muted-foreground))",
              }}
            >
              <Wifi size={16} />
            </div>
            <div className="flex-1 min-w-0">
              <p className="text-[13px] font-medium text-foreground">Server</p>
              <p className="text-[12px] text-muted-foreground mt-0.5 leading-relaxed truncate">
                {connection.serverUrl}
              </p>
            </div>
            <div className="flex-shrink-0 ml-4">
              <button
                onClick={() => void handleDisconnect()}
                className="text-[12px] font-semibold px-3 py-1.5 rounded-lg flex items-center gap-1.5 transition-colors"
                style={{
                  background: "hsl(0 60% 16%)",
                  color: "hsl(0 75% 72%)",
                }}
              >
                <LogOut size={11} />
                Disconnect
              </button>
            </div>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="mb-7">
      <p className="section-label px-1 mb-2.5 flex items-center gap-2">
        <span
          className="inline-block w-1 h-1 rounded-full"
          style={{ background: "hsl(var(--accent-violet))" }}
        />
        Enterprise
      </p>
      <div className="panel p-5">
        <EnterpriseConnectForm
          compact
          onConnected={(conn) => {
            setConnection(conn);
          }}
        />
      </div>
    </div>
  );
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
  performanceMonitorEnabled?: boolean;
  onPerformanceMonitorChange?: (enabled: boolean) => void;
  onEnterpriseDisconnect?: () => void;
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
  performanceMonitorEnabled = false,
  onPerformanceMonitorChange,
  onEnterpriseDisconnect,
}: SettingsViewProps) {
  // Helper — settings are always section-scoped in the stable UI. If a caller
  // omits the section, default to Models instead of rendering every advanced
  // panel at once.
  const currentSection = activeSection ?? "models";
  const isOn    = (id: SettingsSection) => currentSection === id;
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
  const [voicePrompt,      setVoicePrompt]      = useState<PromptTemplateResponse | null>(null);
  const [promptDraftBody,  setPromptDraftBody]  = useState("");
  const [promptBodyDirty,  setPromptBodyDirty]  = useState(false);
  const [promptBusy,       setPromptBusy]       = useState(false);
  const [promptError,      setPromptError]      = useState("");
  const [promptTestInput,  setPromptTestInput]  = useState("Get my task, please.");
  const [promptTestResult, setPromptTestResult] = useState<PromptTestResponse | null>(null);
  const [promptTesting,    setPromptTesting]    = useState(false);
  const [promptPreviewMode, setPromptPreviewMode] = useState<PromptPreviewMode>("rendered");
  const promptLoadStartedRef = useRef(false);

  // ── API key state ────────────────────────────────────────────────────────────
  const [gatewayKey,    setGatewayKey]    = useState("");
  const [deepgramKey,   setDeepgramKey]   = useState("");
  const [geminiKey,     setGeminiKey]     = useState("");
  const [groqKey,       setGroqKey]       = useState("");
  const [cerebrasKey,   setCerebrasKey]   = useState("");
  const [showGateway,   setShowGateway]   = useState(false);
  const [showDeepgram,  setShowDeepgram]  = useState(false);
  const [showGemini,    setShowGemini]    = useState(false);
  const [showGroq,      setShowGroq]      = useState(false);
  const [, setShowCerebras]  = useState(false);
  const [keySaving,     setKeySaving]     = useState(false);
  const [keySaved,      setKeySaved]      = useState(false);
  const keySaveTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  // ── Debug logs state ───────────────────────────────────────────────────────
  const [debugLogs,    setDebugLogs]    = useState<DebugLogs | null>(null);
  const [debugBusy,    setDebugBusy]    = useState(false);
  const [debugCopied,  setDebugCopied]  = useState<"combined" | "desktop" | "backend" | null>(null);

  // ── Desktop-only prefs (Sentry + channel — read at process startup) ─────────
  // These live in <data_dir>/desktop_prefs.json and require an app restart
  // to take effect. UI shows that explicitly.
  const [desktopPrefs, setDesktopPrefsState] = useState<DesktopPrefs>({
    sentry_disabled: false,
    update_channel: "stable",
  });
  useEffect(() => {
    void getDesktopPrefs().then(setDesktopPrefsState).catch(() => {});
  }, []);
  const writeDesktopPrefs = useCallback((next: DesktopPrefs) => {
    setDesktopPrefsState(next);
    void setDesktopPrefs(next).catch((e) => {
      console.warn("[settings] failed to write desktop prefs:", e);
    });
  }, []);
  const [debugTab,     setDebugTab]     = useState<"combined" | "desktop" | "backend">("combined");

  // ── Auto-update state ─────────────────────────────────────────────────────
  const [appVersion, setAppVersion] = useState("…");
  const [updateStatus, setUpdateStatus] = useState<"idle" | "checking" | "available" | "downloading" | "ready" | "up-to-date" | "error">("idle");
  const [updateVersion, setUpdateVersion] = useState("");
  const [updateError, setUpdateError] = useState("");

  // ── Developer log state ───────────────────────────────────────────────────
  const [devLog, setDevLog] = useState("");
  const [devLogPath, setDevLogPath] = useState("");
  const [devLogLoading, setDevLogLoading] = useState(false);

  const loadDevLog = useCallback(async () => {
    setDevLogLoading(true);
    try {
      const [text, path] = await Promise.all([readBackendLog(800), backendLogLocation()]);
      setDevLog(text || "(log is empty)");
      setDevLogPath(path);
    } catch (err) {
      setDevLog(err instanceof Error ? err.message : String(err));
    } finally {
      setDevLogLoading(false);
    }
  }, []);

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

  const [downloadProgress, setDownloadProgress] = useState(0);

  const downloadUpdateNow = useCallback(async () => {
    setUpdateStatus("downloading");
    setDownloadProgress(0);
    try {
      let downloaded = 0;
      let total = 0;
      // Download + verify only — do NOT install here. On Windows installing
      // would close the app immediately; we defer it to the Restart button so
      // the UX matches macOS (download in-app, relaunch to apply).
      const version = await downloadUpdate((event) => {
        if (event.event === "Started") {
          total = (event.data as { contentLength?: number }).contentLength ?? 0;
          downloaded = 0;
        } else if (event.event === "Progress") {
          downloaded += (event.data as { chunkLength: number }).chunkLength;
          if (total > 0) setDownloadProgress(Math.round((downloaded / total) * 100));
        } else if (event.event === "Finished") {
          setDownloadProgress(100);
        }
      });
      if (!version) return;
      setUpdateStatus("ready");
    } catch (err) {
      setUpdateError(err instanceof Error ? err.message : String(err));
      setUpdateStatus("error");
    }
  }, []);
  const recordHotkey = prefs?.record_hotkey ?? "caps_lock";
  // Hotkey labels are platform-aware: the same `right_option` pref maps to
  // VK_RMENU (Right Alt) on Windows. The Fn / Globe key has no PC analog,
  // so it's hidden from the picker on Windows entirely.
  const isWindows = snapshot?.platform === "windows";
  const recordHotkeyLabel =
    recordHotkey === "right_option" ? (isWindows ? "Right Alt" : "Right Option") :
    recordHotkey === "fn" ? "Fn" :
    "Caps Lock";

  function syncApiKeyInputs(nextPrefs: Preferences) {
    setGatewayKey(nextPrefs.gateway_api_key ?? "");
    setDeepgramKey(nextPrefs.deepgram_api_key ?? "");
    setGeminiKey(nextPrefs.gemini_api_key ?? "");
    setGroqKey(nextPrefs.groq_api_key ?? "");
    setCerebrasKey(nextPrefs.cerebras_api_key ?? "");
    setShowGateway(false);
    setShowDeepgram(false);
    setShowGemini(false);
    setShowGroq(false);
    setShowCerebras(false);
  }

  useEffect(() => {
    getVersion().then(setAppVersion).catch(() => setAppVersion("?"));
  }, []);

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
      const currentCerebras = prefs.cerebras_api_key ?? "";
      const nextGateway = gatewayKey.trim();
      const nextDeepgram = deepgramKey.trim();
      const nextGemini = geminiKey.trim();
      const nextGroq = groqKey.trim();
      const nextCerebras = cerebrasKey.trim();

      if (nextGateway !== currentGateway) update.gateway_api_key = nextGateway || null;
      if (nextDeepgram !== currentDeepgram) update.deepgram_api_key = nextDeepgram || null;
      if (nextGemini !== currentGemini) update.gemini_api_key = nextGemini || null;
      if (nextGroq !== currentGroq) update.groq_api_key = nextGroq || null;
      if (nextCerebras !== currentCerebras) update.cerebras_api_key = nextCerebras || null;
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

  function syncVoicePrompt(template: PromptTemplateResponse) {
    setVoicePrompt(template);
    setPromptDraftBody(template.draft_body ?? template.active_body);
    setPromptBodyDirty(false);
  }

  async function loadVoicePrompt() {
    promptLoadStartedRef.current = true;
    setPromptBusy(true);
    setPromptError("");
    try {
      const template = await getVoicePrompt();
      if (!template) throw new Error("Prompt template is unavailable");
      syncVoicePrompt(template);
    } catch (err) {
      setPromptError(err instanceof Error ? err.message : "Failed to load prompt");
    } finally {
      setPromptBusy(false);
    }
  }

  async function savePromptDraft() {
    if (!promptDraftBody.trim()) {
      setPromptError("Prompt draft cannot be empty");
      return;
    }
    setPromptBusy(true);
    setPromptError("");
    try {
      const template = await saveVoicePromptDraft(promptDraftBody);
      if (!template) throw new Error("Backend did not return the saved prompt");
      syncVoicePrompt(template);
    } catch (err) {
      setPromptError(err instanceof Error ? err.message : "Failed to save draft");
    } finally {
      setPromptBusy(false);
    }
  }

  async function applyPromptDraft() {
    if (!promptDraftBody.trim()) {
      setPromptError("Prompt draft cannot be empty");
      return;
    }
    setPromptBusy(true);
    setPromptError("");
    try {
      if (promptBodyDirty) {
        const saved = await saveVoicePromptDraft(promptDraftBody);
        if (!saved) throw new Error("Backend did not save the draft");
      }
      const template = await applyVoicePromptDraft();
      if (!template) throw new Error("Backend did not apply the prompt");
      syncVoicePrompt(template);
    } catch (err) {
      setPromptError(err instanceof Error ? err.message : "Failed to apply prompt");
    } finally {
      setPromptBusy(false);
    }
  }

  async function resetPromptToDefault() {
    if (!window.confirm("Reset the active voice prompt to the app default?")) return;
    setPromptBusy(true);
    setPromptError("");
    try {
      const template = await resetVoicePrompt();
      if (!template) throw new Error("Backend did not reset the prompt");
      syncVoicePrompt(template);
      setPromptTestResult(null);
    } catch (err) {
      setPromptError(err instanceof Error ? err.message : "Failed to reset prompt");
    } finally {
      setPromptBusy(false);
    }
  }

  async function runPromptTest() {
    if (!promptTestInput.trim()) {
      setPromptError("Add a sample transcript first");
      return;
    }
    setPromptTesting(true);
    setPromptError("");
    setPromptTestResult(null);
    try {
      const result = await testVoicePrompt(promptTestInput, promptDraftBody);
      if (!result) throw new Error("Backend did not return a prompt test result");
      setPromptTestResult(result);
    } catch (err) {
      setPromptError(err instanceof Error ? err.message : "Prompt test failed");
    } finally {
      setPromptTesting(false);
    }
  }

  useEffect(() => {
    if (isOn("debug") && !debugLogs && !debugBusy) {
      refreshDebugLogs();
    }
  }, [activeSection]);

  useEffect(() => {
    if (isOn("writing") && !voicePrompt && !promptBusy && !promptLoadStartedRef.current) {
      loadVoicePrompt();
    }
  }, [activeSection, voicePrompt, promptBusy]);

  const tone = (prefs?.tone_preset ?? "neutral") as ToneKey;
  const activeToneLabel = TONE_PRESETS.find((preset) => preset.key === tone)?.label ?? "Neutral";
  const hasStoredGeminiKey = Boolean(prefs?.gemini_api_key);
  const learningEnabled = prefs?.learning_enabled ?? true;
  const promptDiff = useMemo(
    () => diffPromptLines(voicePrompt?.active_body ?? "", promptDraftBody),
    [voicePrompt?.active_body, promptDraftBody]
  );
  const renderedPromptPreview = useMemo(
    () => renderPromptTemplatePreview(
      promptDraftBody,
      tone,
      customPrompt,
      prefs?.output_language ?? "hinglish"
    ),
    [promptDraftBody, tone, customPrompt, prefs?.output_language]
  );
  const promptChangedFromActive = Boolean(voicePrompt && promptDraftBody !== voicePrompt.active_body);
  const promptCanApply = promptBodyDirty || promptChangedFromActive || Boolean(voicePrompt?.has_draft);

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

        {/* ── Appearance ────────────────────────────────── */}
        <Show when={isOn("appearance")}>
          <div className="mb-7">
            <AppearanceSection />
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
                    aria-pressed={isActive}
                    onClick={() => patch({ tone_preset: t.key })}
                    className="text-left px-3 py-2.5 rounded-xl transition-all"
                    style={{
                      background: isActive
                        ? "hsl(var(--primary) / 0.14)"
                        : "hsl(var(--surface-4))",
                      color: isActive
                        ? "hsl(var(--foreground))"
                        : "hsl(var(--muted-foreground))",
                      boxShadow: isActive
                        ? "inset 0 0 0 1px hsl(var(--primary) / 0.65)"
                        : "inset 0 0 0 1px transparent",
                    }}
                  >
                    <p className="text-[12px] font-semibold leading-tight flex items-center justify-between gap-2">
                      {t.label}
                      {isActive && <Check size={12} />}
                    </p>
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

          {/* Voice prompt editor */}
          <div className="panel p-4 mt-3">
            <div className="flex flex-wrap items-center justify-between gap-3 mb-3">
              <div className="min-w-0">
                <div className="flex items-center gap-2">
                  <GitCompareArrows size={14} className="text-muted-foreground" />
                  <p className="text-[12px] font-semibold text-foreground">Prompt Lab</p>
                  {voicePrompt?.has_draft && (
                    <span
                      className="text-[10px] font-semibold px-2 py-0.5 rounded-full"
                      style={{ background: "hsl(var(--surface-4))", color: "hsl(var(--muted-foreground))" }}
                    >
                      Draft saved
                    </span>
                  )}
                </div>
                <p className="text-[11px] text-muted-foreground mt-0.5">
                  Active prompt updates the next recording after Apply.
                </p>
              </div>
              <div className="flex items-center gap-2">
                {voicePrompt && (
                  <span className="text-[10px] text-muted-foreground hidden sm:inline">
                    Applied {formatPromptTime(voicePrompt.applied_at)}
                  </span>
                )}
                <button
                  type="button"
                  onClick={loadVoicePrompt}
                  disabled={promptBusy}
                  className="btn-ghost !py-1.5 !px-3"
                >
                  {promptBusy ? <Loader2 size={12} className="animate-spin" /> : <RefreshCw size={12} />}
                  Reload
                </button>
              </div>
            </div>

            {promptError && (
              <div
                className="mb-3 rounded-lg px-3 py-2 text-[12px]"
                style={{ background: "hsl(0 70% 14%)", color: "hsl(0 85% 76%)" }}
              >
                {promptError}
              </div>
            )}

            {voicePrompt ? (
              <div className="space-y-3">
                <div className="grid grid-cols-1 lg:grid-cols-2 gap-3">
                  <div>
                    <div className="flex items-center justify-between mb-1.5">
                      <p className="text-[11px] font-semibold text-muted-foreground">Active</p>
                      <span className="text-[10px] text-muted-foreground">
                        {voicePrompt.active_is_default ? "Default" : voicePrompt.base_version}
                      </span>
                    </div>
                    <textarea
                      readOnly
                      value={voicePrompt.active_body}
                      rows={13}
                      className="input resize-none font-mono text-[11px] leading-relaxed opacity-80"
                    />
                  </div>

                  <div>
                    <div className="flex items-center justify-between mb-1.5">
                      <p className="text-[11px] font-semibold text-muted-foreground">Draft</p>
                      <span className="text-[10px] text-muted-foreground">
                        {promptChangedFromActive ? "Changed" : "Same as active"}
                      </span>
                    </div>
                    <textarea
                      value={promptDraftBody}
                      onChange={(e) => {
                        setPromptDraftBody(e.target.value);
                        setPromptBodyDirty(true);
                        setPromptTestResult(null);
                      }}
                      rows={13}
                      className="input resize-none font-mono text-[11px] leading-relaxed"
                      spellCheck={false}
                    />
                  </div>
                </div>

                <div className="flex flex-wrap items-center justify-between gap-2">
                  <div className="flex flex-wrap items-center gap-2">
                    <button
                      type="button"
                      onClick={() => {
                        setPromptDraftBody(voicePrompt.active_body);
                        setPromptBodyDirty(false);
                        setPromptTestResult(null);
                      }}
                      className="btn-ghost !py-1.5 !px-3"
                    >
                      Use active
                    </button>
                    <button
                      type="button"
                      onClick={() => {
                        setPromptDraftBody(voicePrompt.default_body);
                        setPromptBodyDirty(true);
                        setPromptTestResult(null);
                      }}
                      className="btn-ghost !py-1.5 !px-3"
                    >
                      Use default
                    </button>
                    <button
                      type="button"
                      onClick={resetPromptToDefault}
                      disabled={promptBusy}
                      className="btn-ghost !py-1.5 !px-3"
                    >
                      <RotateCcw size={12} />
                      Reset active
                    </button>
                  </div>
                  <div className="flex items-center gap-2">
                    <button
                      type="button"
                      onClick={savePromptDraft}
                      disabled={promptBusy || !promptBodyDirty}
                      className="btn-ghost !py-1.5 !px-3"
                    >
                      {promptBusy ? <Loader2 size={12} className="animate-spin" /> : <Save size={12} />}
                      Save draft
                    </button>
                    <button
                      type="button"
                      onClick={applyPromptDraft}
                      disabled={promptBusy || !promptCanApply}
                      className="btn-primary !py-1.5 !px-3 !text-[12px]"
                    >
                      Apply prompt
                    </button>
                  </div>
                </div>

                <div
                  className="rounded-xl p-3"
                  style={{ background: "hsl(var(--surface-4))" }}
                >
                  <div className="flex flex-wrap items-center justify-between gap-2 mb-2">
                    <div>
                      <p className="text-[11px] font-semibold text-muted-foreground">
                        {promptPreviewMode === "rendered" ? "Rendered Prompt" : "Template Diff"}
                      </p>
                      <p className="text-[10px] text-muted-foreground mt-0.5">
                        {promptPreviewMode === "rendered"
                          ? `${activeToneLabel} · ${prefs?.output_language ?? "hinglish"}`
                          : "Active → draft"}
                      </p>
                    </div>
                    <div
                      className="flex rounded-xl p-0.5 gap-0.5"
                      style={{ background: "hsl(var(--surface-2))" }}
                    >
                      {([
                        { key: "rendered", label: "Rendered" },
                        { key: "diff", label: "Diff" },
                      ] as const).map((mode) => {
                        const isModeActive = promptPreviewMode === mode.key;
                        return (
                          <button
                            key={mode.key}
                            type="button"
                            onClick={() => setPromptPreviewMode(mode.key)}
                            className="text-[11px] font-medium rounded-[10px] px-3 py-1.5 transition-all"
                            style={{
                              background: isModeActive ? "hsl(var(--surface-1))" : "transparent",
                              color: isModeActive ? "hsl(var(--foreground))" : "hsl(var(--muted-foreground))",
                            }}
                          >
                            {mode.label}
                          </button>
                        );
                      })}
                    </div>
                  </div>

                  {promptPreviewMode === "rendered" ? (
                    <textarea
                      readOnly
                      value={renderedPromptPreview}
                      rows={13}
                      className="input resize-none font-mono text-[11px] leading-relaxed"
                    />
                  ) : (
                    <div
                      className="max-h-64 overflow-auto rounded-lg font-mono text-[11px] leading-relaxed"
                      style={{ background: "hsl(var(--surface-2))" }}
                    >
                      {promptDiff.map((line, index) => (
                        <div
                          key={`${line.type}-${index}`}
                          className="grid grid-cols-[24px_1fr] gap-2 px-2 py-0.5 whitespace-pre-wrap break-words"
                          style={{
                            color:
                              line.type === "add"
                                ? "hsl(145 70% 70%)"
                                : line.type === "remove"
                                ? "hsl(0 80% 76%)"
                                : "hsl(var(--muted-foreground))",
                            background:
                              line.type === "add"
                                ? "hsl(145 70% 24% / 0.25)"
                                : line.type === "remove"
                                ? "hsl(0 70% 24% / 0.25)"
                                : "transparent",
                          }}
                        >
                          <span>{line.type === "add" ? "+" : line.type === "remove" ? "-" : " "}</span>
                          <span>{line.text || " "}</span>
                        </div>
                      ))}
                    </div>
                  )}
                </div>

                <div
                  className="rounded-xl p-3"
                  style={{ background: "hsl(var(--surface-4))" }}
                >
                  <div className="flex items-center justify-between gap-3 mb-2">
                    <p className="text-[11px] font-semibold text-muted-foreground">Test Sample</p>
                    <button
                      type="button"
                      onClick={runPromptTest}
                      disabled={promptTesting || promptBusy || !promptTestInput.trim()}
                      className="btn-primary !py-1.5 !px-3 !text-[12px]"
                    >
                      {promptTesting ? <Loader2 size={12} className="animate-spin" /> : <Play size={12} />}
                      Run test
                    </button>
                  </div>
                  <textarea
                    value={promptTestInput}
                    onChange={(e) => setPromptTestInput(e.target.value)}
                    rows={2}
                    className="input resize-none text-[12px] leading-relaxed"
                  />
                  {promptTestResult && (
                    <div className="mt-3 rounded-lg p-3" style={{ background: "hsl(var(--surface-2))" }}>
                      <div className="flex items-center justify-between gap-3 mb-1.5">
                        <p className="text-[11px] font-semibold text-muted-foreground">Output</p>
                        <span className="text-[10px] text-muted-foreground">
                          {promptTestResult.model_used} · {promptTestResult.latency_ms}ms
                        </span>
                      </div>
                      <p className="text-[13px] text-foreground leading-relaxed whitespace-pre-wrap">
                        {promptTestResult.output}
                      </p>
                    </div>
                  )}
                </div>
              </div>
            ) : (
              <div
                className="rounded-xl px-4 py-6 flex items-center justify-center gap-2 text-[12px] text-muted-foreground"
                style={{ background: "hsl(var(--surface-4))" }}
              >
                <Loader2 size={14} className={cn(promptBusy && "animate-spin")} />
                {promptBusy ? "Loading prompt…" : "Prompt not loaded"}
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
                { key: "right_option", label: isWindows ? "Right Alt" : "Right Option" },
                // Fn / Globe key has no Windows analog — hide the option there.
                ...(!isWindows ? [{ key: "fn", label: "Fn" }] : []),
              ] as { key: "caps_lock" | "right_option" | "fn"; label: string }[]).map((opt) => {
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

        {/* ── Models ───────────────────────────────────── */}
        <Show when={isOn("models")}>
        <Section title="Dictation Model">
          <div className="px-5 py-4">
            <div className="flex items-center gap-4 mb-3">
              <div
                className="w-9 h-9 rounded-xl flex items-center justify-center flex-shrink-0 text-muted-foreground"
                style={{ background: "hsl(var(--surface-4))" }}
              >
                <Zap size={16} />
              </div>
              <div className="min-w-0 flex-1">
                <p className="text-[13px] font-medium text-foreground">Normal voice polish</p>
                <p className="text-[12px] text-muted-foreground mt-0.5">
                  Groq is the fixed provider. Choose speed vs quality for regular hotkey dictation.
                </p>
              </div>
            </div>
            <div
              className="flex rounded-xl p-1"
              style={{ background: "hsl(var(--surface-3))" }}
            >
              {([
                {
                  key: "fast",
                  icon: <Zap size={13} />,
                  label: "Fast",
                  desc: "Llama 3.1 8B - lowest latency",
                },
                {
                  key: "smart",
                  icon: <Sparkles size={13} />,
                  label: "Smart",
                  desc: "Llama 4 Scout - better for complex sentences",
                },
              ] as const).map((opt) => {
                const isActive = (prefs?.selected_model ?? "fast") === opt.key;
                return (
                  <button
                    key={opt.key}
                    onClick={() => void patch({ selected_model: opt.key })}
                    className="flex-1 text-left px-3 py-2.5 rounded-[10px] transition-all"
                    style={{
                      background: isActive ? "hsl(var(--surface-1))" : "transparent",
                      boxShadow: isActive
                        ? "0 1px 3px hsl(0 0% 0% / 0.3)"
                        : "none",
                    }}
                  >
                    <p
                      className="text-[12px] font-semibold leading-tight flex items-center justify-between gap-2"
                      style={{
                        color: isActive
                          ? "hsl(var(--foreground))"
                          : "hsl(var(--muted-foreground))",
                      }}
                    >
                      <span className="flex items-center gap-1.5">
                        {opt.icon}
                        {opt.label}
                      </span>
                      {isActive && <Check size={12} />}
                    </p>
                    <p
                      className="text-[10px] leading-snug mt-0.5"
                      style={{ color: "hsl(var(--muted-foreground))" }}
                    >
                      {opt.desc}
                    </p>
                  </button>
                );
              })}
            </div>
          </div>
        </Section>

        <ChatGPTSection />
        </Show>

        {/* ── Notifications ─────────────────────────────── */}
        <Show when={isOn("notifications")}>
        <div className="mb-7">
          <p className="section-label px-1 mb-2.5">Status Bar Notifications</p>
          <NotificationToggles />
        </div>
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
                <p>• <strong>Microphone</strong> — lets AirNote record your voice.</p>
              )}
              {!axGranted && (
                <p>• <strong>Accessibility</strong> — lets AirNote paste text directly into any app.</p>
              )}
              {!imGranted && (
                <p>• <strong>Input Monitoring</strong> — lets AirNote listen for the {recordHotkeyLabel} recording hotkey.</p>
              )}
              <p className="mt-1.5 opacity-70">
                After granting a permission, return to AirNote. This page updates automatically.
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
                    ? "Granted — AirNote can record your voice."
                    : isWindows
                    ? "Required for dictation. Windows asks the first time you start recording."
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

            {/* On Windows we hide the Accessibility row entirely: SendInput
                + global keyboard hooks need no system permission, so the
                concept doesn't apply. Macs keep the row + divider. */}
            {!isWindows && (
              <>
                <div className="mx-5 border-t" style={{ borderColor: "hsl(var(--surface-3))" }} />
                <div className="flex items-center gap-4 px-5 py-4">
                  <div
                    className="w-9 h-9 rounded-xl flex items-center justify-center flex-shrink-0"
                    style={{
                      background: "hsl(var(--surface-4))",
                      color: "hsl(var(--muted-foreground))",
                    }}
                  >
                    <Shield size={16} />
                  </div>
                  <div className="flex-1 min-w-0">
                    <p className="text-[13px] font-medium text-foreground">Accessibility</p>
                    <p className="text-[12px] text-muted-foreground mt-0.5 leading-relaxed">
                      {axGranted
                        ? "Granted — AirNote can paste text into any app."
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
              </>
            )}

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
                    ? "Granted — AirNote will notify you when a learning edit is ready to review."
                    : notifPerm === "denied"
                    ? isWindows
                      ? "Denied — open Windows Settings → System → Notifications & actions → AirNote to enable."
                      : "Denied — open System Settings → Notifications → AirNote to enable."
                    : "AirNote asks once to send learning-edit notifications."}
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

            {/* Input Monitoring row — Mac-only concept. WH_KEYBOARD_LL needs no
                grant on Windows, so the row is hidden there entirely. */}
            {!isWindows && (
              <>
                <div className="mx-5 border-t" style={{ borderColor: "hsl(var(--surface-3))" }} />
                <div className="flex items-center gap-4 px-5 py-4">
                  <div
                    className="w-9 h-9 rounded-xl flex items-center justify-center flex-shrink-0"
                    style={{
                      background: "hsl(var(--surface-4))",
                      color: "hsl(var(--muted-foreground))",
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
              </>
            )}

          </div>
        </div>
        </Show>

        {/* ── API Keys ──────────────────────────────────── */}
        <Show when={isOn("api-keys")}>
        <div className="mb-7">
          <p className="section-label px-1 mb-2.5">API Keys</p>
          <div className="panel p-5 space-y-3">
            <p className="text-[12px] text-muted-foreground leading-relaxed">
              Stored only on this {isWindows ? "PC" : "Mac"}. Open a group only when you need to edit that key.
            </p>

            <SettingsDisclosure
              title="Required for dictation"
              description="Groq polishes text. Deepgram transcribes audio."
              icon={<Zap size={15} />}
              defaultOpen
              status={
                prefs?.groq_api_key && prefs?.deepgram_api_key ? (
                  <span className="text-[10px] px-2 py-1 rounded-full"
                        style={{ background: "hsl(145 60% 16%)", color: "hsl(145 70% 65%)" }}>
                    Ready
                  </span>
                ) : (
                  <span className="text-[10px] px-2 py-1 rounded-full"
                        style={{ background: "hsl(30 80% 20%)", color: "hsl(30 90% 75%)" }}>
                    Missing
                  </span>
                )
              }
            >
              <SecretInput
                icon={<Zap size={12} className="text-muted-foreground" />}
                label="Groq API Key"
                helper="Used by normal voice polish, repair, classification, and fallbacks."
                placeholder="gsk_..."
                value={groqKey}
                visible={showGroq}
                onChange={setGroqKey}
                onToggle={() => setShowGroq((v) => !v)}
              />
              <SecretInput
                icon={<Cpu size={12} className="text-muted-foreground" />}
                label="Deepgram API Key"
                helper="Used for speech-to-text."
                placeholder="Token ..."
                value={deepgramKey}
                visible={showDeepgram}
                onChange={setDeepgramKey}
                onToggle={() => setShowDeepgram((v) => !v)}
              />
            </SettingsDisclosure>

            <SettingsDisclosure
              title="Optional learning"
              description="Gemini embeddings improve local vocabulary/context memory."
              icon={<Sparkles size={15} />}
              status={
                prefs?.gemini_api_key ? (
                  <span className="text-[10px] px-2 py-1 rounded-full"
                        style={{ background: "hsl(var(--surface-4))", color: "hsl(var(--muted-foreground))" }}>
                    Configured
                  </span>
                ) : undefined
              }
            >
              <SecretInput
                icon={<Sparkles size={12} className="text-muted-foreground" />}
                label="Gemini API Key"
                helper="Optional. Without it, dictation still works."
                placeholder="AIza..."
                value={geminiKey}
                visible={showGemini}
                onChange={setGeminiKey}
                onToggle={() => setShowGemini((v) => !v)}
              />

              <div className="flex items-center justify-between gap-4 rounded-xl px-3 py-2.5"
                   style={{ background: "hsl(var(--surface-4))" }}>
                <div className="min-w-0">
                  <p className="text-[12px] font-semibold text-foreground">Enable smart learning</p>
                  <p className="text-[11px] text-muted-foreground mt-0.5">
                    Uses local corrections plus embeddings when a Gemini key is saved.
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
                      : "hsl(var(--surface-3))",
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
            </SettingsDisclosure>

            <SettingsDisclosure
              title="Advanced legacy gateway"
              description="Only needed for older routing setups. Most users should leave this closed."
              icon={<Wifi size={15} />}
              status={
                prefs?.gateway_api_key ? (
                  <span className="text-[10px] px-2 py-1 rounded-full"
                        style={{ background: "hsl(var(--surface-4))", color: "hsl(var(--muted-foreground))" }}>
                    Set
                  </span>
                ) : undefined
              }
            >
              <SecretInput
                icon={<Wifi size={12} className="text-muted-foreground" />}
                label="Gateway API Key"
                helper="Legacy/shared gateway. Not required for the current Groq-first path."
                placeholder="sk-..."
                value={gatewayKey}
                visible={showGateway}
                onChange={setGatewayKey}
                onToggle={() => setShowGateway((v) => !v)}
              />
            </SettingsDisclosure>

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
        </Show>

        {/* ── Enterprise ──────────────────────────────── */}
        <Show when={isOn("enterprise")}>
          <EnterpriseSection onDisconnect={onEnterpriseDisconnect} />
        </Show>

        {/* ── Debug ───────────────────────────────────── */}
        <Show when={isOn("debug")}>
        <Section title="Performance">
          <Row
            icon={<Activity size={16} />}
            label="Sidebar monitor"
            description="Show live CPU, memory, process usage, and GPU availability while testing lag."
            action={
              <button
                type="button"
                role="switch"
                aria-checked={performanceMonitorEnabled}
                onClick={() => onPerformanceMonitorChange?.(!performanceMonitorEnabled)}
                className="relative h-6 w-11 rounded-full transition-colors"
                style={{
                  background: performanceMonitorEnabled
                    ? "hsl(var(--primary))"
                    : "hsl(var(--surface-4))",
                }}
              >
                <span
                  className="absolute top-1 h-4 w-4 rounded-full transition-transform"
                  style={{
                    left: 4,
                    transform: performanceMonitorEnabled
                      ? "translateX(20px)"
                      : "translateX(0)",
                    background: "hsl(var(--foreground))",
                  }}
                />
              </button>
            }
          />
        </Section>

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
                  ["desktop",  "AirNote"],
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
                    {debugBusy ? "Loading" : debugTab === "combined" ? "Combined" : debugTab === "desktop" ? "AirNote desktop" : "airnote-backend"}
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
            label={`AirNote v${appVersion}`}
            description="Voice Polish Studio · Local-first · Tauri + Rust + React"
          />

          {/* Diagnostics toggle — Sentry, opt-out. Requires restart. */}
          <Row
            icon={<Bug size={16} />}
            label="Send anonymous diagnostics"
            description={
              desktopPrefs.sentry_disabled
                ? "Off — zero telemetry leaves your machine. See PRIVACY.md."
                : "On — anonymous crash reports + error logs. No content, no audio, no API keys. Restart to apply changes."
            }
            action={
              <button
                type="button"
                role="switch"
                aria-checked={!desktopPrefs.sentry_disabled}
                onClick={() => writeDesktopPrefs({
                  ...desktopPrefs,
                  sentry_disabled: !desktopPrefs.sentry_disabled,
                })}
                className="relative h-6 w-11 rounded-full transition-colors"
                style={{
                  background: !desktopPrefs.sentry_disabled
                    ? "hsl(var(--primary))"
                    : "hsl(var(--surface-4))",
                }}
              >
                <span
                  className="absolute top-1 h-4 w-4 rounded-full transition-transform"
                  style={{
                    left: 4,
                    transform: !desktopPrefs.sentry_disabled
                      ? "translateX(20px)"
                      : "translateX(0)",
                    background: "hsl(var(--foreground))",
                  }}
                />
              </button>
            }
          />

          {/* Update channel toggle — stable / beta. Beta is a no-op in v3.0
              until manifests-branch publishing lands; the pref still persists. */}
          <Row
            icon={<GitCompareArrows size={16} />}
            label="Update channel"
            description={
              desktopPrefs.update_channel === "beta"
                ? "Beta — preview builds when available. Pref stored; runtime endpoint switch ships in v3.x."
                : "Stable — recommended for most users."
            }
            action={
              <div className="flex items-center gap-1 rounded-md p-0.5"
                   style={{ background: "hsl(var(--surface-4))" }}>
                {(["stable", "beta"] as const).map((ch) => (
                  <button
                    key={ch}
                    type="button"
                    onClick={() => writeDesktopPrefs({ ...desktopPrefs, update_channel: ch })}
                    className="px-2.5 py-1 rounded text-[11px] font-medium transition-colors"
                    style={{
                      background: desktopPrefs.update_channel === ch
                        ? "hsl(var(--primary))"
                        : "transparent",
                      color: desktopPrefs.update_channel === ch
                        ? "hsl(var(--background))"
                        : "hsl(var(--muted-foreground))",
                    }}
                  >
                    {ch === "stable" ? "Stable" : "Beta"}
                  </button>
                ))}
              </div>
            }
          />

          <Row
            icon={<Download size={16} />}
            label="Software Update"
            description={
              updateStatus === "checking" ? "Checking for updates…" :
              updateStatus === "available" ? `Version ${updateVersion} is available` :
              updateStatus === "downloading" ? `Downloading update… ${downloadProgress}%` :
              updateStatus === "ready" ? "Update downloaded — relaunch to finish" :
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
                    onClick={() => void downloadUpdateNow()}
                    className="px-3 py-1 rounded-md text-[11px] font-medium border border-transparent text-background"
                    style={{ background: "hsl(var(--primary))" }}
                  >
                    Install Update
                  </button>
                ) : updateStatus === "ready" ? (
                  <button
                    onClick={() => void applyPendingUpdate()}
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

          {isWindows && (
            <div className="mt-4">
              <div className="flex items-center justify-between mb-2">
                <p className="section-label px-1">Developer log</p>
                <div className="flex items-center gap-2">
                  <button
                    type="button"
                    onClick={() => void loadDevLog()}
                    className="px-3 py-1 rounded-md text-[11px] font-medium border border-border text-muted-foreground hover:text-foreground hover:border-foreground/30 transition-all"
                  >
                    {devLogLoading ? "Loading…" : "Refresh"}
                  </button>
                  <button
                    type="button"
                    onClick={() => void openLogFolder()}
                    className="px-3 py-1 rounded-md text-[11px] font-medium border border-border text-muted-foreground hover:text-foreground hover:border-foreground/30 transition-all"
                  >
                    Open folder
                  </button>
                  {devLog && (
                    <button
                      type="button"
                      onClick={() => void navigator.clipboard.writeText(devLog)}
                      className="px-3 py-1 rounded-md text-[11px] font-medium border border-border text-muted-foreground hover:text-foreground hover:border-foreground/30 transition-all"
                    >
                      Copy
                    </button>
                  )}
                </div>
              </div>
              <p className="text-[11px] text-muted-foreground px-1 mb-2 break-all">
                {devLogPath || "Backend daemon log (backend.log) — tail of the latest entries."}
              </p>
              <pre className="text-[10.5px] leading-relaxed font-mono whitespace-pre-wrap break-words bg-muted/40 border border-border rounded-md p-3 max-h-72 overflow-auto">
                {devLog || "Click Refresh to load the latest backend log."}
              </pre>
            </div>
          )}
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

// ── Notification Toggles ──────────────────────────────────────────────────────

const NOTIF_STORAGE_KEY = "airnote-notif-prefs";

interface NotifPrefs {
  learned: boolean;
  queued: boolean;
  confirm: boolean;
  negative: boolean;
  retrain: boolean;
  updates: boolean;
  error: boolean;
  sounds: boolean;
}

const DEFAULT_NOTIF: NotifPrefs = {
  learned: true,
  queued: true,
  confirm: true,
  negative: true,
  retrain: true,
  updates: true,
  error: true,
  sounds: true,
};

function loadNotifPrefs(): NotifPrefs {
  try {
    const raw = localStorage.getItem(NOTIF_STORAGE_KEY);
    if (raw) return { ...DEFAULT_NOTIF, ...JSON.parse(raw) };
  } catch { /* ignore */ }
  return { ...DEFAULT_NOTIF };
}

function saveNotifPrefs(p: NotifPrefs) {
  try { localStorage.setItem(NOTIF_STORAGE_KEY, JSON.stringify(p)); } catch { /* ignore */ }
}

// Exported so StatusBar.tsx can read these prefs
export function getNotifPrefs(): NotifPrefs { return loadNotifPrefs(); }

const NOTIF_ITEMS: { key: keyof NotifPrefs; label: string; desc: string }[] = [
  { key: "learned",  label: "Word learned",          desc: "When a new vocabulary term is added or a new spelling is recorded" },
  { key: "queued",   label: "Correction noticed",    desc: "When a correction is queued but not yet confirmed (sighting 1/3)" },
  { key: "confirm",  label: "Ambiguous term",        desc: "Ask whether a corrected word is a brand/name (one-click confirm)" },
  { key: "negative", label: "Wrong correction",      desc: "Alert when AirNote keeps making the same wrong correction" },
  { key: "retrain",  label: "Model updated",         desc: "When the ONNX correction model finishes retraining" },
  { key: "updates",  label: "App update ready",      desc: "When AirNote has downloaded an update and needs a restart" },
  { key: "error",    label: "Errors",                desc: "Recording errors, backend connection issues" },
  { key: "sounds",   label: "Sound effects",         desc: "Play subtle sounds on recording start, paste, learning events" },
];

function NotificationToggles() {
  const [prefs, setPrefs] = useState<NotifPrefs>(loadNotifPrefs);

  function toggle(key: keyof NotifPrefs) {
    setPrefs(prev => {
      const next = { ...prev, [key]: !prev[key] };
      saveNotifPrefs(next);
      return next;
    });
  }

  return (
    <div className="panel p-4 space-y-1">
      <p className="text-[12px] mb-3" style={{ color: "hsl(var(--muted-foreground))" }}>
        Choose which notifications appear in the floating status bar.
      </p>
      {NOTIF_ITEMS.map(item => (
        <div
          key={item.key}
          className="flex items-center justify-between py-2.5 px-1"
          style={{ borderBottom: "1px solid hsl(var(--border) / 0.5)" }}
        >
          <div className="flex-1 min-w-0">
            <div className="text-[13px] font-medium" style={{ color: "hsl(var(--foreground))" }}>
              {item.label}
            </div>
            <div className="text-[11px] mt-0.5" style={{ color: "hsl(var(--muted-foreground))" }}>
              {item.desc}
            </div>
          </div>
          <button
            onClick={() => toggle(item.key)}
            className="ml-3 w-9 h-5 rounded-full transition-colors flex-shrink-0 relative"
            style={{
              background: prefs[item.key]
                ? "hsl(var(--primary))"
                : "hsl(var(--surface-4))",
            }}
          >
            <span
              className="block w-3.5 h-3.5 rounded-full bg-white absolute top-[3px] transition-transform"
              style={{
                transform: prefs[item.key] ? "translateX(17px)" : "translateX(3px)",
              }}
            />
          </button>
        </div>
      ))}
    </div>
  );
}
