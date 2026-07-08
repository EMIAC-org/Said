import React, { useCallback, useEffect, useState } from "react";
import { ScrollArea } from "@/components/ui/scroll-area";
import { cn } from "@/lib/utils";
import { formatKeycap, hotkeyDisplay, type Platform } from "@/lib/hotkeys";
import { getVersion } from "@tauri-apps/api/app";
import {
  Shield, Key, Info, Wifi, Check, Sparkles,
  Languages, MessageSquareText, Loader2, RefreshCw,
  Bell, Bug, Copy, FileText, Mic, Download, Activity,
  Save, GitCompareArrows, Link, LogOut, Power, BookOpen,
  Code2, Plus, Trash2, AlertTriangle, Monitor,
} from "lucide-react";
import { check } from "@tauri-apps/plugin-updater";
import { applyPendingUpdate, downloadUpdate, getPendingReadyUpdateVersion } from "@/lib/autoUpdate";
import type { AppSnapshot, Preferences } from "@/types";
import { AppearanceSection } from "@/components/views/AppearanceSection";
import type { Theme } from "@/lib/useTheme";
import { DictationSttSection } from "@/components/DictationSttSection";
import { HotkeyPicker } from "@/components/HotkeyPicker";

import {
  getConnection as enterpriseGetConnection,
  disconnectEnterprise,
  getServerUrlMode,
  getServerUrlOverride,
  getActiveServerUrl,
  applyServerUrlConfig,
  DEFAULT_CLOUD_SERVER_URL,
  type EnterpriseConnection,
  type ServerUrlMode,
} from "@/lib/enterprise";
import { EnterpriseConnectForm } from "@/components/EnterpriseConnectForm";
import {
  getPreferences, patchPreferences,
  getDebugLogs,
  requestNotifications, checkNotificationPermission,
  getDesktopPrefs, setDesktopPrefs, requestBrowserAutomation,
  browserAutomationStatus, triggerBrowserAutomation, type BrowserAutomation,
  readBackendLog, backendLogLocation, openLogFolder,
  openExternal,
  getServerSettingsStatus,
  getDeveloperSettings,
  saveDeveloperSettings,
  developerProblemBegin,
  developerProblemEnd,
  emptyDeveloperSettings,
  type DebugLogs,
  type NotifPermission,
  type DesktopPrefs,
  type ServerSettingsStatus,
  type DeveloperSettings,
  type DeveloperProjectProfile,
  type DeveloperProfileWarning,
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

const DEVELOPER_CONTEXT_MAX_CHARS = 8_000;

function splitDeveloperAliases(value: string): string[] {
  return value
    .split(/[\n,]/g)
    .map((alias) => alias.trim())
    .filter(Boolean);
}

function createDeveloperProfile(): DeveloperProjectProfile {
  const id =
    typeof crypto !== "undefined" && "randomUUID" in crypto
      ? crypto.randomUUID()
      : `profile-${Date.now()}`;
  return {
    id,
    name: "New Project",
    aliases: [],
    context: "",
    enabled: true,
    source_type: "manual",
    updated_at: Date.now(),
  };
}

// ── Language options ──────────────────────────────────────────────────────────

const LANGUAGES = [
  { key: "auto",  label: "Auto (Hindi + English)" },
  { key: "hi",    label: "Hindi" },
  { key: "multi", label: "Hindi + English (code-switching)" },
  { key: "en",    label: "English" },
  { key: "en-IN", label: "English (India)" },
];

// ── Sub-components ────────────────────────────────────────────────────────────

type SyncBadgeState = "idle" | "syncing" | "synced" | "offline" | "failed";

function SyncBadge({ state }: { state: SyncBadgeState }) {
  if (state === "idle") return null;
  const configs: Record<Exclude<SyncBadgeState, "idle">, { label: string; fg: string; bg: string }> = {
    synced:  { label: "Synced",        fg: "hsl(var(--chip-cyan-fg))",  bg: "hsl(var(--chip-cyan-bg))"  },
    syncing: { label: "Syncing…",      fg: "hsl(var(--chip-amber-fg))", bg: "hsl(var(--chip-amber-bg))" },
    offline: { label: "Offline cache", fg: "hsl(var(--chip-amber-fg))", bg: "hsl(var(--chip-amber-bg))" },
    failed:  { label: "Sync failed",   fg: "hsl(var(--chip-red-fg))",   bg: "hsl(var(--chip-red-bg))"   },
  };
  const cfg = configs[state as Exclude<SyncBadgeState, "idle">];
  if (!cfg) return null;
  return (
    <span
      className="ml-2 inline-flex items-center gap-1.5 px-2 py-0.5 rounded-full text-[10px] font-semibold"
      style={{ color: cfg.fg, background: cfg.bg }}
    >
      <span
        className="inline-block w-1.5 h-1.5 rounded-full flex-shrink-0"
        style={{ background: "currentColor" }}
      />
      {cfg.label}
    </span>
  );
}

function Section({
  title,
  extra,
  children,
}: {
  title: string;
  extra?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <div className="mb-7">
      <p className="section-label px-1 mb-2.5 flex items-center gap-2">
        <span
          className="inline-block w-1 h-1 rounded-full"
          style={{ background: "hsl(var(--accent-violet))" }}
        />
        {title}
        {extra}
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
  | "appearance"
  | "writing"
  | "hotkeys"
  | "models"
  | "developer"
  | "notifications"
  | "permissions"
  | "enterprise"
  | "debug"
  | "about";

export const SETTINGS_SECTIONS: { id: SettingsSection; label: string }[] = [
  { id: "appearance",     label: "Appearance"     },
  { id: "hotkeys",      label: "Hotkeys"      },
  { id: "models",         label: "Models"         },
  // Developer section hidden from the user-facing nav. The section type, panel,
  // and all handlers are intentionally kept — re-add this entry to surface it.
  // { id: "developer",      label: "Developer"      },
  { id: "notifications",  label: "Notifications"  },
  { id: "permissions",    label: "Permissions"     },
  { id: "enterprise",  label: "Enterprise"    },
  { id: "about",       label: "About"         },
];

function Show({ when, children }: { when: boolean; children: React.ReactNode }) {
  return when ? <>{children}</> : null;
}

// ── Enterprise section ────────────────────────────────────────────────────────

/** Server URL override — toggle between the default (prod AirNote) backend and a
 *  custom URL. The active URL governs the control-plane AND the local backend's
 *  polish forwarding. Applying reloads the app so every endpoint re-resolves. */
function ServerOverrideCard() {
  const [mode, setMode] = useState<ServerUrlMode>(() => getServerUrlMode());
  const [customUrl, setCustomUrl] = useState<string>(() => getServerUrlOverride());
  const [busy, setBusy] = useState(false);
  const active = getActiveServerUrl();

  async function apply(nextMode: ServerUrlMode, nextUrl?: string) {
    setBusy(true);
    try {
      await applyServerUrlConfig(nextMode, nextUrl);
      window.location.reload(); // reload so cached endpoints pick up the new URL
    } catch {
      setBusy(false);
    }
  }

  const segStyle = (on: boolean) => ({
    background: on ? "hsl(var(--primary))" : "hsl(var(--muted))",
    color: on ? "hsl(var(--primary-foreground))" : "hsl(var(--foreground))",
    boxShadow: on ? "0 2px 8px -4px hsl(var(--primary) / 0.45)" : "inset 0 0 0 1px hsl(var(--border))",
  });

  return (
    <div className="mb-7">
      <p className="section-label px-1 mb-2.5 flex items-center gap-2">
        <span className="inline-block w-1 h-1 rounded-full" style={{ background: "hsl(var(--accent-violet))" }} />
        Workspace server
      </p>
      <div className="panel p-5 space-y-3">
        <p className="text-[12px] text-muted-foreground leading-relaxed">
          Where AirNote sends sign-in and dictation requests. Changing this reloads the app and may require signing in again.
        </p>
        <div className="flex flex-wrap gap-2">
          <button
            type="button"
            disabled={busy}
            onClick={() => { setMode("default"); if (mode !== "default") void apply("default"); }}
            className="text-[11.5px] font-semibold px-3 py-1.5 rounded-full transition-colors disabled:opacity-50"
            style={segStyle(mode === "default")}
          >
            Default (prod AirNote)
          </button>
          <button
            type="button"
            disabled={busy}
            onClick={() => setMode("custom")}
            className="text-[11.5px] font-semibold px-3 py-1.5 rounded-full transition-colors disabled:opacity-50"
            style={segStyle(mode === "custom")}
          >
            Custom
          </button>
        </div>

        {mode === "custom" && (
          <div className="flex items-center gap-2">
            <input
              type="url"
              value={customUrl}
              disabled={busy}
              onChange={(e) => setCustomUrl(e.target.value)}
              placeholder="https://your-server.example.com"
              className="input flex-1 text-[12px] font-mono"
            />
            <button
              type="button"
              disabled={busy || !customUrl.trim()}
              onClick={() => void apply("custom", customUrl)}
              className="btn-primary !py-1.5 !px-4 !text-[12px] flex-shrink-0"
            >
              Done
            </button>
          </div>
        )}

        <p className="text-[11px] text-muted-foreground">
          Active:{" "}
          <span className="font-mono" style={{ color: "hsl(var(--foreground) / 0.8)" }}>
            {active}
          </span>
          {getServerUrlMode() === "default" && (
            <span className="ml-1 opacity-60">· {DEFAULT_CLOUD_SERVER_URL.replace(/^https?:\/\//, "")}</span>
          )}
        </p>
      </div>
    </div>
  );
}

function EnterpriseSection({ onDisconnect }: { onDisconnect?: () => void }) {
  const [connection, setConnection] = useState<EnterpriseConnection | null>(null);
  const [workspaces, setWorkspaces] = useState<
    import("@/lib/enterprise").WorkspaceMembership[]
  >([]);
  const [personalMode, setPersonalMode] = useState(false);
  const [workspaceBusy, setWorkspaceBusy] = useState(false);

  useEffect(() => {
    setConnection(enterpriseGetConnection());
  }, []);

  useEffect(() => {
    if (!connection) return;
    void (async () => {
      const { listWorkspaces } = await import("@/lib/enterprise");
      const data = await listWorkspaces();
      if (data) {
        setWorkspaces(data.orgs);
        setPersonalMode(data.personal_mode);
      }
    })();
  }, [connection]);

  async function handleDisconnect() {
    await disconnectEnterprise();
    setConnection(null);
    onDisconnect?.();
  }

  if (connection) {
    return (
      <>
      <ServerOverrideCard />
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
              className="w-9 h-9 rounded-xl flex items-center justify-center flex-shrink-0 overflow-hidden settings-disclosure__icon"
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
                  className={
                    connection.authSource === "email"
                      ? "status-pill chip-blue"
                      : "status-pill--ready"
                  }
                >
                  {connection.authSource === "email" ? "Email only" : "Connected"}
                </span>
              </div>
              <p className="text-[12px] text-muted-foreground mt-0.5 leading-relaxed truncate">
                {connection.larkName
                  ? `${connection.larkName} · ${connection.email}`
                  : connection.email}
              </p>
            </div>
          </div>

          {workspaces.length > 0 && (
            <>
              <div className="mx-5 border-t" style={{ borderColor: "hsl(var(--border))" }} />
              <div className="px-5 py-4 space-y-2.5">
                <p className="text-[11px] font-semibold text-muted-foreground uppercase tracking-[0.06em]">
                  Active workspace
                </p>
                <div className="flex flex-wrap gap-2">
                  <button
                    type="button"
                    disabled={workspaceBusy}
                    onClick={() => {
                      void (async () => {
                        setWorkspaceBusy(true);
                        const { deactivateWorkspace, listWorkspaces } = await import("@/lib/enterprise");
                        if (await deactivateWorkspace()) {
                          const data = await listWorkspaces();
                          if (data) {
                            setWorkspaces(data.orgs);
                            setPersonalMode(true);
                          }
                        }
                        setWorkspaceBusy(false);
                      })();
                    }}
                    className="text-[11px] font-semibold px-3 py-1.5 rounded-full transition-colors disabled:opacity-50"
                    style={{
                      background: personalMode ? "hsl(var(--primary))" : "hsl(var(--muted))",
                      color: personalMode ? "hsl(var(--primary-foreground))" : "hsl(var(--foreground))",
                      boxShadow: personalMode
                        ? "0 2px 8px -4px hsl(var(--primary) / 0.45)"
                        : "inset 0 0 0 1px hsl(var(--border))",
                    }}
                  >
                    Personal
                  </button>
                  {workspaces.map((org) => (
                    <button
                      key={org.id}
                      type="button"
                      disabled={workspaceBusy}
                      onClick={() => {
                        void (async () => {
                          setWorkspaceBusy(true);
                          const { activateWorkspace, listWorkspaces } = await import("@/lib/enterprise");
                          if (await activateWorkspace(org.id)) {
                            const data = await listWorkspaces();
                            if (data) {
                              setWorkspaces(data.orgs);
                              setPersonalMode(data.personal_mode);
                            }
                            setConnection(enterpriseGetConnection());
                          }
                          setWorkspaceBusy(false);
                        })();
                      }}
                      className="text-[11px] font-semibold px-3 py-1.5 rounded-full transition-colors disabled:opacity-50"
                      style={{
                        background: org.is_active ? "hsl(var(--primary))" : "hsl(var(--muted))",
                        color: org.is_active ? "hsl(var(--primary-foreground))" : "hsl(var(--foreground))",
                        boxShadow: org.is_active
                          ? "0 2px 8px -4px hsl(var(--primary) / 0.45)"
                          : "inset 0 0 0 1px hsl(var(--border))",
                      }}
                    >
                      {org.name}
                    </button>
                  ))}
                </div>
              </div>
            </>
          )}

          <div className="mx-5 border-t" style={{ borderColor: "hsl(var(--border))" }} />

          <div className="flex items-center gap-4 px-5 py-4">
            <div
              className="settings-disclosure__icon w-9 h-9 rounded-xl flex items-center justify-center flex-shrink-0"
            >
              <Wifi size={16} />
            </div>
            <div className="flex-1 min-w-0">
              <p className="text-[13px] font-medium text-foreground">Server</p>
              <p className="text-[12px] text-muted-foreground mt-0.5 leading-relaxed truncate font-mono">
                {connection.serverUrl}
              </p>
            </div>
            <div className="flex-shrink-0 ml-4">
              <button
                onClick={() => void handleDisconnect()}
                className="btn-soft-danger"
              >
                <LogOut size={11} />
                Disconnect
              </button>
            </div>
          </div>
        </div>
      </div>
      </>
    );
  }

  return (
    <>
      <ServerOverrideCard />
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
    </>
  );
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
  performanceMonitorEnabled?: boolean;
  onPerformanceMonitorChange?: (enabled: boolean) => void;
  onEnterpriseDisconnect?: () => void;
  /** Active theme + setter for the Appearance theme picker. */
  theme?:            Theme;
  onThemeChange?:    (t: Theme) => void;
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
  performanceMonitorEnabled = false,
  onPerformanceMonitorChange,
  onEnterpriseDisconnect,
  theme,
  onThemeChange,
}: SettingsViewProps) {
  // Helper — settings are always section-scoped in the stable UI. If a caller
  // omits the section, default to Models instead of rendering every advanced
  // panel at once.
  const currentSection = activeSection ?? "models";
  const isOn    = (id: SettingsSection) => currentSection === id;
  const axGranted  = snapshot?.accessibility_granted    ?? false;
  const imGranted  = snapshot?.input_monitoring_granted ?? false;
  const micGranted = snapshot?.microphone_granted       ?? false;
  const screenGranted = snapshot?.screen_recording_granted ?? false;

  const [notifPerm, setNotifPerm] = useState<NotifPermission>("unknown");
  const [notifBusy, setNotifBusy] = useState(false);
  const axSupported = snapshot?.auto_paste_supported    ?? false;

  // ── Server settings sync indicator ──────────────────────────────────────────
  const [serverSyncState, setServerSyncState] = useState<SyncBadgeState>("idle");
  useEffect(() => {
    if (!isOn("models")) return;
    void getServerSettingsStatus().then((s: ServerSettingsStatus | null) => {
      if (!s || !s.signed_in) { setServerSyncState("idle"); return; }
      if (s.last_error)        { setServerSyncState("failed");  return; }
      if (s.synced)            { setServerSyncState("synced");  return; }
      setServerSyncState("offline");
    }).catch(() => setServerSyncState("idle"));
  }, [currentSection]); // eslint-disable-line react-hooks/exhaustive-deps

  // ── Prefs state ─────────────────────────────────────────────────────────────
  const [prefs,        setPrefs]        = useState<Preferences | null>(null);
  const [saving,       setSaving]       = useState(false);
  const [saveError,    setSaveError]    = useState("");
  const [customPrompt, setCustomPrompt] = useState("");
  const [promptDirty,  setPromptDirty]  = useState(false);

  // ── API key state ────────────────────────────────────────────────────────────
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
    message_polish_mode: false,
    launch_at_login: false,
    beta_mode: false,
    browser_context_enabled: false,
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
  // Live per-browser Automation consent state (Apple Events), so the browser-
  // context row can show granted/denied/not-asked and a working Grant button.
  const [browserAuto, setBrowserAuto] = useState<BrowserAutomation[]>([]);
  const refreshBrowserAuto = useCallback(async () => {
    setBrowserAuto(await browserAutomationStatus());
  }, []);
  useEffect(() => {
    if (desktopPrefs.browser_context_enabled) void refreshBrowserAuto();
  }, [desktopPrefs.browser_context_enabled, refreshBrowserAuto]);
  const [debugTab,     setDebugTab]     = useState<"combined" | "desktop" | "backend">("combined");

  // ── Auto-update state ─────────────────────────────────────────────────────
  const [appVersion, setAppVersion] = useState("…");
  const [updateStatus, setUpdateStatus] = useState<"idle" | "checking" | "available" | "downloading" | "ready" | "applying" | "up-to-date" | "error">("idle");
  const [updateVersion, setUpdateVersion] = useState("");
  const [updateError, setUpdateError] = useState("");

  // ── Developer log state ───────────────────────────────────────────────────
  const [devLog, setDevLog] = useState("");
  const [devLogPath, setDevLogPath] = useState("");
  const [devLogLoading, setDevLogLoading] = useState(false);

  // ── Developer Problem Command state ──────────────────────────────────────
  const [developerSettings, setDeveloperSettings] = useState<DeveloperSettings>(() => emptyDeveloperSettings());
  const [developerWarnings, setDeveloperWarnings] = useState<DeveloperProfileWarning[]>([]);
  const [developerLoaded, setDeveloperLoaded] = useState(false);
  const [developerDirty, setDeveloperDirty] = useState(false);
  const [developerBusy, setDeveloperBusy] = useState(false);
  const [developerError, setDeveloperError] = useState("");

  const updateDeveloperDraft = useCallback((updater: (prev: DeveloperSettings) => DeveloperSettings) => {
    setDeveloperSettings((prev) => updater(prev));
    setDeveloperDirty(true);
    setDeveloperError("");
  }, []);

  const saveDeveloperDraft = useCallback(async (settings = developerSettings) => {
    setDeveloperBusy(true);
    setDeveloperError("");
    try {
      const response = await saveDeveloperSettings(settings);
      setDeveloperSettings(response.settings);
      setDeveloperWarnings(response.warnings);
      setDeveloperDirty(false);
    } catch (err) {
      setDeveloperError(err instanceof Error ? err.message : String(err));
    } finally {
      setDeveloperBusy(false);
    }
  }, [developerSettings]);

  const addDeveloperProfile = useCallback(() => {
    updateDeveloperDraft((prev) => ({
      ...prev,
      profiles: [...prev.profiles, createDeveloperProfile()],
    }));
  }, [updateDeveloperDraft]);

  const updateDeveloperProfile = useCallback((
    profileId: string,
    patchProfile: (profile: DeveloperProjectProfile) => DeveloperProjectProfile,
  ) => {
    updateDeveloperDraft((prev) => ({
      ...prev,
      profiles: prev.profiles.map((profile) =>
        profile.id === profileId
          ? { ...patchProfile(profile), updated_at: Date.now() }
          : profile,
      ),
    }));
  }, [updateDeveloperDraft]);

  const removeDeveloperProfile = useCallback((profileId: string) => {
    updateDeveloperDraft((prev) => ({
      ...prev,
      profiles: prev.profiles.filter((profile) => profile.id !== profileId),
    }));
  }, [updateDeveloperDraft]);

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
      if (!version) {
        setUpdateStatus("up-to-date");
        return;
      }
      setUpdateVersion(version);
      setUpdateStatus("ready");
    } catch (err) {
      setUpdateError(err instanceof Error ? err.message : String(err));
      setUpdateStatus("error");
    }
  }, []);

  const relaunchToApplyUpdate = useCallback(async () => {
    setUpdateStatus("applying");
    setUpdateError("");
    try {
      await applyPendingUpdate();
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
  const platform = (snapshot?.platform ?? "macos") as Platform;
  const recordHotkeyLabel = hotkeyDisplay(recordHotkey, platform).label;
  // Polish chord shown per-platform (cmd+shift+p → Ctrl+Shift+P on Windows).
  const polishHotkeyLabel = formatKeycap(prefs?.polish_text_hotkey ?? "cmd+shift+p", platform);



  useEffect(() => {
    getVersion().then(setAppVersion).catch(() => setAppVersion("?"));
  }, []);

  useEffect(() => {
    void getPendingReadyUpdateVersion().then((version) => {
      if (!version) return;
      setUpdateVersion(version);
      setUpdateStatus("ready");
    }).catch(() => {});
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
        if (!p.learning_enabled) {
          patchPreferences({ learning_enabled: true }).then((updated) => {
            if (updated) setPrefs(updated);
          });
        }
      }
    });
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

  async function patch(update: Partial<Preferences>) {
    if (!prefs) return;
    const prev = prefs;
    setSaving(true);
    setSaveError("");
    try {
      const updated = await patchPreferences(update);
      if (!updated) {
        setPrefs(prev);
        setSaveError("Failed to save — is the backend running?");
        return;
      }
      setPrefs(updated);
    } catch (err) {
      setPrefs(prev);
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

  useEffect(() => {
    if (!isOn("developer") || developerLoaded || developerBusy) return;
    let alive = true;
    setDeveloperBusy(true);
    setDeveloperError("");
    getDeveloperSettings()
      .then((response) => {
        if (!alive) return;
        setDeveloperSettings(response.settings);
        setDeveloperWarnings(response.warnings);
        setDeveloperLoaded(true);
        setDeveloperDirty(false);
      })
      .catch((err) => {
        if (!alive) return;
        setDeveloperLoaded(true);
        setDeveloperError(err instanceof Error ? err.message : String(err));
      })
      .finally(() => {
        if (alive) setDeveloperBusy(false);
      });
    return () => {
      alive = false;
    };
  }, [activeSection, developerLoaded, developerBusy]);

  const beginDeveloperCommandTest = useCallback(() => {
    developerProblemBegin().catch((err) => {
      setDeveloperError(err instanceof Error ? err.message : String(err));
    });
  }, []);

  const endDeveloperCommandTest = useCallback(() => {
    developerProblemEnd().catch((err) => {
      setDeveloperError(err instanceof Error ? err.message : String(err));
    });
  }, []);

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

        {/* ── Appearance ────────────────────────────────── */}
        <Show when={isOn("appearance")}>
          <div className="mb-7">
            <AppearanceSection theme={theme} onThemeChange={onThemeChange} />
          </div>
        </Show>

        {/* ── Hotkeys ─────────────────────────────────── */}
        <Show when={isOn("hotkeys")}>
        <Section title="Voice Hotkey">
          <div className="px-5 py-4">
            <div className="flex items-center gap-4">
              <div
                className="w-9 h-9 rounded-xl flex items-center justify-center flex-shrink-0 text-muted-foreground"
                style={{ background: "hsl(var(--surface-4))" }}
              >
                <Key size={16} />
              </div>
              <div className="flex-1">
                <p className="text-[13px] font-medium text-foreground mb-1">Hold-to-speak key</p>
                <p className="text-[12px] text-muted-foreground">
                  Choose the key AirNote listens for when you speak. Changes apply immediately.
                </p>
              </div>
            </div>
            <div className="mt-3">
              <HotkeyPicker
                value={recordHotkey}
                onChange={(id) => patch({ record_hotkey: id })}
                platform={platform}
                disabled={saving}
              />
            </div>
            {saveError && (
              <p className="text-[11px] mt-2" style={{ color: "hsl(var(--destructive))" }}>
                {saveError}
              </p>
            )}
          </div>
        </Section>
        <Section title="Startup">
          <Row
            icon={<Power size={16} />}
            label="Launch at login"
            description={
              desktopPrefs.launch_at_login
                ? "On — AirNote starts automatically when you sign in."
                : "Off — open AirNote manually when you need it."
            }
            action={
              <button
                type="button"
                role="switch"
                aria-checked={desktopPrefs.launch_at_login}
                onClick={() => writeDesktopPrefs({
                  ...desktopPrefs,
                  launch_at_login: !desktopPrefs.launch_at_login,
                })}
                className="relative h-6 w-11 rounded-full transition-colors"
                style={{
                  background: desktopPrefs.launch_at_login
                    ? "hsl(var(--primary))"
                    : "hsl(var(--surface-4))",
                }}
              >
                <span
                  className="absolute top-1 h-4 w-4 rounded-full transition-transform"
                  style={{
                    left: 4,
                    transform: desktopPrefs.launch_at_login
                      ? "translateX(20px)"
                      : "translateX(0)",
                    background: "hsl(var(--foreground))",
                  }}
                />
              </button>
            }
          />
        </Section>
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
              className="mt-3"
            >
              <HotkeyPicker
                value={recordHotkey}
                onChange={(id) => patch({ record_hotkey: id })}
                platform={platform}
              />
            </div>
          </div>
        </Section>
        </Show>

        {/* ── Models ───────────────────────────────────── */}
        <Show when={isOn("models")}>
        {/* On-device STT is cross-platform, so the cloud-vs-local picker shows on Windows too. */}
        <DictationSttSection
          prefs={prefs}
          onPrefsUpdated={setPrefs}
          platform={snapshot?.platform ?? "macos"}
        />
        {/* Dictation polish card hidden from the user-facing settings. Kept (not
            deleted) and still type-checked — change `false` to `true` to restore. */}
        {false && (
          <Section title="Dictation polish" extra={<SyncBadge state={serverSyncState} />}>
            <div className="px-5 py-4">
              <div className="flex items-center gap-4">
                <div
                  className="w-9 h-9 rounded-xl flex items-center justify-center flex-shrink-0 text-muted-foreground"
                  style={{ background: "hsl(var(--surface-4))" }}
                >
                  <Sparkles size={16} />
                </div>
                <div className="min-w-0 flex-1">
                  <p className="text-[13px] font-medium text-foreground">Gemma 4 31B (Cerebras)</p>
                  <p className="text-[12px] text-muted-foreground mt-0.5">
                    {polishHotkeyLabel} polish always runs on airnote.emiactech.com using Cerebras Gemma 4 31B.
                    Speech recognition stays local on this device.
                  </p>
                </div>
              </div>
            </div>
          </Section>
        )}

        </Show>

        {/* ── Developer Problem Command ─────────────────── */}
        <Show when={isOn("developer")}>
        <Section title="Developer problem command">
          <Row
            icon={<Code2 size={16} />}
            label="Enable add-on"
            description={
              developerSettings.enabled
                ? "On — problem requests use their own isolated solve flow."
                : "Off — normal dictation, polish, retry, Divo, and meetings stay unchanged."
            }
            action={
              <button
                type="button"
                role="switch"
                aria-checked={developerSettings.enabled}
                disabled={developerBusy}
                onClick={() => updateDeveloperDraft((prev) => ({ ...prev, enabled: !prev.enabled }))}
                className="relative h-6 w-11 rounded-full transition-colors disabled:opacity-50"
                style={{
                  background: developerSettings.enabled
                    ? "hsl(var(--primary))"
                    : "hsl(var(--surface-4))",
                }}
              >
                <span
                  className="absolute top-1 h-4 w-4 rounded-full transition-transform"
                  style={{
                    left: 4,
                    transform: developerSettings.enabled ? "translateX(20px)" : "translateX(0)",
                    background: "hsl(var(--foreground))",
                  }}
                />
              </button>
            }
          />
          <div className="mx-5 border-t" style={{ borderColor: "hsl(var(--surface-3))" }} />
          <div className="px-5 py-4">
            <div className="flex items-start gap-4">
              <div
                className="w-9 h-9 rounded-xl flex items-center justify-center flex-shrink-0 text-muted-foreground"
                style={{ background: "hsl(var(--surface-4))" }}
              >
                <Mic size={16} />
              </div>
              <div className="flex-1 min-w-0">
                <p className="text-[13px] font-medium text-foreground">Manual hold trigger</p>
                <p className="text-[12px] text-muted-foreground mt-0.5 leading-relaxed">
                  Hold to record a developer problem. It transcribes first, resolves project context locally, then pastes only the final answer.
                </p>
                <div className="mt-3 flex flex-wrap items-center gap-2">
                  <button
                    type="button"
                    disabled={!developerSettings.enabled || developerDirty || developerBusy}
                    onMouseDown={beginDeveloperCommandTest}
                    onMouseUp={endDeveloperCommandTest}
                    onMouseLeave={endDeveloperCommandTest}
                    onTouchStart={beginDeveloperCommandTest}
                    onTouchEnd={endDeveloperCommandTest}
                    className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-[12px] font-semibold transition disabled:opacity-45"
                    style={{
                      background: "hsl(var(--primary))",
                      color: "hsl(var(--primary-foreground))",
                    }}
                  >
                    <Mic size={12} />
                    Hold Developer Command
                  </button>
                  {developerDirty && (
                    <span className="text-[11px] text-muted-foreground">
                      Save settings before testing
                    </span>
                  )}
                </div>
              </div>
            </div>
          </div>
        </Section>

        <Section
          title="Project context profiles"
          extra={
            <span className="ml-2 text-[10px] font-semibold px-2 py-0.5 rounded-full"
              style={{ color: "hsl(var(--muted-foreground))", background: "hsl(var(--surface-4))" }}>
              Local only
            </span>
          }
        >
          <div className="px-5 py-4 space-y-4">
            <div className="flex items-start justify-between gap-3">
              <div>
                <p className="text-[13px] font-medium text-foreground">Context is a short project gist</p>
                <p className="text-[12px] text-muted-foreground mt-0.5 leading-relaxed">
                  Keep it to stack, architecture, known modules, conventions, and current goals. V1 does not learn from edits or sync wiki pages.
                </p>
              </div>
              <button
                type="button"
                onClick={addDeveloperProfile}
                className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-[12px] font-semibold transition"
                style={{ background: "hsl(var(--surface-4))", color: "hsl(var(--foreground))" }}
              >
                <Plus size={12} />
                Add
              </button>
            </div>

            {developerSettings.profiles.length === 0 ? (
              <div
                className="rounded-xl px-4 py-5 text-center text-[12px] text-muted-foreground"
                style={{ background: "hsl(var(--surface-2))" }}
              >
                Add a project profile to let spoken aliases select context.
              </div>
            ) : (
              <div className="space-y-3">
                {developerSettings.profiles.map((profile) => {
                  const contextCount = profile.context.length;
                  const overLimit = contextCount > DEVELOPER_CONTEXT_MAX_CHARS;
                  const profileWarnings = developerWarnings.filter((warning) => warning.profile_id === profile.id);
                  return (
                    <div
                      key={profile.id}
                      className="rounded-xl p-4"
                      style={{
                        background: "hsl(var(--surface-2))",
                        border: "1px solid hsl(var(--border) / 0.55)",
                      }}
                    >
                      <div className="flex items-start gap-3">
                        <div className="flex-1 min-w-0 grid grid-cols-1 md:grid-cols-2 gap-3">
                          <label className="block">
                            <span className="text-[11px] font-semibold text-muted-foreground uppercase tracking-[0.05em]">
                              Project name
                            </span>
                            <input
                              value={profile.name}
                              onChange={(e) => updateDeveloperProfile(profile.id, (p) => ({ ...p, name: e.target.value }))}
                              className="mt-1 w-full rounded-lg px-3 py-2 text-[13px] outline-none"
                              style={{
                                background: "hsl(var(--surface-1))",
                                color: "hsl(var(--foreground))",
                                border: "1px solid hsl(var(--border))",
                              }}
                              placeholder="HRM8"
                            />
                          </label>
                          <label className="block">
                            <span className="text-[11px] font-semibold text-muted-foreground uppercase tracking-[0.05em]">
                              Aliases
                            </span>
                            <textarea
                              value={profile.aliases.join("\n")}
                              onChange={(e) => updateDeveloperProfile(profile.id, (p) => ({
                                ...p,
                                aliases: splitDeveloperAliases(e.target.value),
                              }))}
                              className="mt-1 w-full min-h-[74px] resize-y rounded-lg px-3 py-2 text-[13px] outline-none"
                              style={{
                                background: "hsl(var(--surface-1))",
                                color: "hsl(var(--foreground))",
                                border: "1px solid hsl(var(--border))",
                              }}
                              placeholder={"hrm8\nhrm\nhrm desktop"}
                            />
                          </label>
                        </div>
                        <div className="flex items-center gap-2">
                          <button
                            type="button"
                            role="switch"
                            aria-checked={profile.enabled}
                            onClick={() => updateDeveloperProfile(profile.id, (p) => ({ ...p, enabled: !p.enabled }))}
                            className="relative h-6 w-11 rounded-full transition-colors"
                            title={profile.enabled ? "Profile enabled" : "Profile disabled"}
                            style={{
                              background: profile.enabled
                                ? "hsl(var(--primary))"
                                : "hsl(var(--surface-4))",
                            }}
                          >
                            <span
                              className="absolute top-1 h-4 w-4 rounded-full transition-transform"
                              style={{
                                left: 4,
                                transform: profile.enabled ? "translateX(20px)" : "translateX(0)",
                                background: "hsl(var(--foreground))",
                              }}
                            />
                          </button>
                          <button
                            type="button"
                            onClick={() => removeDeveloperProfile(profile.id)}
                            className="w-8 h-8 rounded-lg inline-flex items-center justify-center transition"
                            style={{ color: "hsl(var(--destructive))", background: "hsl(var(--surface-3))" }}
                            title="Delete profile"
                          >
                            <Trash2 size={14} />
                          </button>
                        </div>
                      </div>

                      <label className="block mt-3">
                        <span className="text-[11px] font-semibold text-muted-foreground uppercase tracking-[0.05em]">
                          Context brief
                        </span>
                        <textarea
                          value={profile.context}
                          maxLength={DEVELOPER_CONTEXT_MAX_CHARS}
                          onChange={(e) => updateDeveloperProfile(profile.id, (p) => ({ ...p, context: e.target.value }))}
                          className="mt-1 w-full min-h-[150px] resize-y rounded-lg px-3 py-2 text-[13px] leading-relaxed outline-none"
                          style={{
                            background: "hsl(var(--surface-1))",
                            color: "hsl(var(--foreground))",
                            border: overLimit
                              ? "1px solid hsl(var(--destructive))"
                              : "1px solid hsl(var(--border))",
                          }}
                          placeholder={"Stack:\nArchitecture:\nKnown modules:\nConventions:\nCurrent goals:"}
                        />
                        <div className="mt-1 flex items-center justify-between gap-2">
                          <span className="text-[11px] text-muted-foreground">
                            {contextCount.toLocaleString()} / {DEVELOPER_CONTEXT_MAX_CHARS.toLocaleString()} characters
                          </span>
                          {profile.enabled ? (
                            <span className="status-pill--ready text-[10px]">Enabled</span>
                          ) : (
                            <span className="status-pill text-[10px]">Disabled</span>
                          )}
                        </div>
                      </label>

                      {profileWarnings.length > 0 && (
                        <div className="mt-3 space-y-1.5">
                          {profileWarnings.map((warning, index) => (
                            <div
                              key={`${warning.alias ?? "profile"}-${index}`}
                              className="flex items-start gap-2 rounded-lg px-3 py-2 text-[11px]"
                              style={{
                                background: "hsl(38 80% 12% / 0.75)",
                                color: "hsl(38 90% 72%)",
                              }}
                            >
                              <AlertTriangle size={12} className="mt-0.5 flex-shrink-0" />
                              <span>{warning.alias ? `${warning.alias}: ` : ""}{warning.message}</span>
                            </div>
                          ))}
                        </div>
                      )}
                    </div>
                  );
                })}
              </div>
            )}

            <div
              className="rounded-xl px-4 py-3 text-[12px] leading-relaxed"
              style={{ background: "hsl(var(--surface-2))", color: "hsl(var(--muted-foreground))" }}
            >
              <p className="font-semibold text-foreground mb-1">Future context source</p>
              <p>Lark Wiki sync stays out of V1. When added later, it should be explicit, permissioned, source-labeled, and versioned.</p>
            </div>

            {developerError && (
              <div
                className="rounded-xl px-4 py-3 text-[12px]"
                style={{ background: "hsl(var(--destructive) / 0.12)", color: "hsl(var(--destructive))" }}
              >
                {developerError}
              </div>
            )}

            <div className="flex items-center justify-between gap-3 pt-1">
              <span className="text-[11px] text-muted-foreground">
                {developerDirty ? "Unsaved developer settings" : developerLoaded ? "Developer settings saved" : "Loading developer settings"}
              </span>
              <button
                type="button"
                disabled={developerBusy || !developerDirty}
                onClick={() => void saveDeveloperDraft()}
                className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-[12px] font-semibold transition disabled:opacity-45"
                style={{ background: "hsl(var(--primary))", color: "hsl(var(--primary-foreground))" }}
              >
                {developerBusy ? <Loader2 size={12} className="animate-spin" /> : <Save size={12} />}
                Save Developer Settings
              </button>
            </div>
          </div>
        </Section>
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

            {/* Screen Recording row — macOS only. Gates ScreenCaptureKit, which
                is how meetings capture system audio. */}
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
                    <Monitor size={16} />
                  </div>
                  <div className="flex-1 min-w-0">
                    <p className="text-[13px] font-medium text-foreground">Screen Recording</p>
                    <p className="text-[12px] text-muted-foreground mt-0.5 leading-relaxed">
                      {screenGranted
                        ? "Granted — AirNote can capture meeting audio."
                        : "Required to record meeting audio (system sound). Opens System Settings → Privacy & Security → Screen Recording. You may need to reopen AirNote after granting."}
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

            {/* Browser context — opt-in, macOS. Reads the active tab's domain
                for site-level dictation context; toggling on triggers the
                per-browser Automation consent prompt. */}
            {!isWindows && (
              <>
                <div className="mx-5 border-t" style={{ borderColor: "hsl(var(--surface-3))" }} />
                <div className="flex items-center gap-4 px-5 py-4">
                  <div
                    className="w-9 h-9 rounded-xl flex items-center justify-center flex-shrink-0"
                    style={{ background: "hsl(var(--surface-4))", color: "hsl(var(--muted-foreground))" }}
                  >
                    <Link size={16} />
                  </div>
                  <div className="flex-1 min-w-0">
                    <p className="text-[13px] font-medium text-foreground">Browser context</p>
                    <p className="text-[12px] text-muted-foreground mt-0.5 leading-relaxed">
                      When you dictate into a browser, remember the site (domain only — e.g.
                      mail.google.com, never the full URL), stored on this Mac only. Turning this
                      on asks macOS for permission to read your browser’s active tab.
                    </p>
                  </div>
                  <div className="flex-shrink-0 ml-4">
                    <button
                      type="button"
                      role="switch"
                      aria-checked={desktopPrefs.browser_context_enabled}
                      onClick={() => {
                        const next = !desktopPrefs.browser_context_enabled;
                        writeDesktopPrefs({ ...desktopPrefs, browser_context_enabled: next });
                        if (next) {
                          void requestBrowserAutomation().then(refreshBrowserAuto);
                        }
                      }}
                      className="relative h-6 w-11 rounded-full transition-colors"
                      style={{
                        background: desktopPrefs.browser_context_enabled
                          ? "hsl(var(--primary))"
                          : "hsl(var(--surface-4))",
                      }}
                    >
                      <span
                        className="absolute top-1 h-4 w-4 rounded-full transition-transform"
                        style={{
                          left: 4,
                          transform: desktopPrefs.browser_context_enabled
                            ? "translateX(20px)"
                            : "translateX(0)",
                          background: "hsl(var(--foreground))",
                        }}
                      />
                    </button>
                  </div>
                </div>

                {/* Live per-browser Automation consent. Apple Events need a
                    running target, so if nothing's open we prompt the user to
                    open a browser; granted browsers show a badge, others a
                    Grant button (denied → System Settings, macOS won't re-ask). */}
                {desktopPrefs.browser_context_enabled && (
                  <div className="mx-5 mb-4 -mt-1">
                    {browserAuto.length === 0 ? (
                      <div
                        className="rounded-xl px-4 py-3 text-[12px] text-muted-foreground leading-relaxed"
                        style={{ background: "hsl(var(--surface-3))" }}
                      >
                        Open a browser (Chrome, Safari, Edge, Brave, Arc…), then{" "}
                        <button
                          type="button"
                          className="underline underline-offset-2 text-foreground"
                          onClick={() => void refreshBrowserAuto()}
                        >
                          refresh
                        </button>{" "}
                        to grant access — macOS asks once per browser.
                      </div>
                    ) : (
                      <div
                        className="rounded-xl overflow-hidden"
                        style={{ background: "hsl(var(--surface-3))" }}
                      >
                        {browserAuto.map((b, i) => (
                          <div
                            key={b.app_key}
                            className="flex items-center gap-3 px-4 py-2.5"
                            style={i > 0 ? { borderTop: "1px solid hsl(var(--surface-4))" } : undefined}
                          >
                            <span className="text-[12px] text-foreground flex-1 min-w-0 truncate">
                              {b.name}
                            </span>
                            {b.status === "granted" ? (
                              <span
                                className="text-[11px] font-medium"
                                style={{ color: "hsl(152 60% 50%)" }}
                              >
                                Granted
                              </span>
                            ) : b.status === "denied" ? (
                              <button
                                type="button"
                                className="text-[11px] font-medium px-2.5 py-1 rounded-lg"
                                style={{ background: "hsl(var(--surface-4))", color: "hsl(var(--foreground))" }}
                                onClick={() =>
                                  void openExternal(
                                    "x-apple.systempreferences:com.apple.preference.security?Privacy_Automation",
                                  )
                                }
                              >
                                Denied — open Settings
                              </button>
                            ) : (
                              <button
                                type="button"
                                className="text-[11px] font-medium px-2.5 py-1 rounded-lg"
                                style={{ background: "hsl(var(--primary))", color: "hsl(var(--primary-foreground))" }}
                                onClick={() =>
                                  void triggerBrowserAutomation(b.app_key).then(refreshBrowserAuto)
                                }
                              >
                                Grant
                              </button>
                            )}
                          </div>
                        ))}
                      </div>
                    )}
                  </div>
                )}
              </>
            )}

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

          {/* User guide — every shortcut, dictation, polish & HUD, in one page. */}
          <Row
            icon={<BookOpen size={16} />}
            label="User Guide & shortcuts"
            description="All hotkeys, dictation, message polish & HUD placement — opens the full guide in your browser."
            action={
              <button
                type="button"
                onClick={() => void openExternal("https://airnote.emiactech.com/guide")}
                className="rounded-lg border px-3 py-1.5 text-[13px] font-medium text-foreground transition-colors hover:bg-[hsl(var(--surface-3))]"
                style={{ borderColor: "hsl(var(--surface-3))" }}
              >
                Open guide
              </button>
            }
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
              updateStatus === "applying" ? "Applying update…" :
              updateStatus === "ready" ? "Update downloaded — relaunch to finish" :
              updateStatus === "up-to-date" ? "You're on the latest version" :
              updateStatus === "error" ? (updateError || "Update check failed") :
              "Check for available updates"
            }
            last
            action={
              <div className="flex items-center gap-2">
                {updateStatus === "checking" || updateStatus === "downloading" || updateStatus === "applying" ? (
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
                    type="button"
                    onClick={() => void relaunchToApplyUpdate()}
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
