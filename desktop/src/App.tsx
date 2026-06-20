import { useEffect, useState, useCallback } from "react";
import { X } from "lucide-react";
import { Sidebar } from "@/components/Sidebar";
import { InviteTeamModal } from "@/components/InviteTeamModal";
import { SettingsModal } from "@/components/SettingsModal";
import { OnboardingFlow } from "@/components/OnboardingFlow";
import { Topbar } from "@/components/Topbar";
import { DashboardView } from "@/components/views/DashboardView";
import { HistoryView } from "@/components/views/HistoryView";
import { InsightsView } from "@/components/views/InsightsView";
import { VocabularyView } from "@/components/views/VocabularyView";
import { MeetingsView } from "@/components/views/MeetingsView";
import { DivoView } from "@/components/views/DivoView";
import { LiveMeetingView } from "@/components/views/LiveMeetingView";
import {
  invoke,
  listHistory,
  onAppState,
  onNavSettings,
  onVoiceDone,
  onVoiceStatus,
  onVoiceToken,
  onVoiceError,
  onEditDetected,
  onPendingEditsChanged,
  getPendingEdits,
  resolvePendingEdit,
  sendNotification,
  requestInputMonitoring,
  requestMicrophone,
  submitEditFeedback,
  onVocabToast,
  onDictationRecovered,
  divoSetCredentials,
  deleteVocabularyTerm,
  checkNotificationPermission,
  revealDownloadedFile,
  getMigrationStatus,
  runMigration,
  syncServerSettings,
  syncCredentialVault,
  type NotifPermission,
  type VocabToastPayload,
  type ServerMigrationStatus,
} from "@/lib/invoke";
import {
  checkConnection,
  getConnection,
  isConnected,
  ensureDesktopRegistered,
  restoreConnectionFromLocalBackend,
  syncCompanyVocab,
  uploadUserVocabSummary,
  type EnterpriseConnection,
} from "@/lib/enterprise";
import { useTheme } from "@/lib/useTheme";
import { useBackendHeartbeat } from "@/lib/useBackendHeartbeat";
import { startDailyAutoUpdateCheck } from "@/lib/autoUpdate";
import { ReconnectingOverlay } from "@/components/ReconnectingOverlay";
import type { AppSnapshot, HistoryItem, PendingEdit, Recording } from "@/types";
import { RetryToast, EditConfirmToast, VocabularyToast, DownloadSuccessToast } from "@/components/NotificationToast";

export type ActiveView = "dashboard" | "history" | "vocabulary" | "insights" | "meetings" | "divo" | "settings" | "live-meeting";
const VALID_VIEWS: ActiveView[] = ["dashboard", "history", "vocabulary", "insights", "meetings", "divo", "settings", "live-meeting"];
type SettingsSectionId =
  | "appearance"
  | "writing"
  | "hotkeys"
  | "models"
  | "meeting"
  | "notifications"
  | "permissions"
  | "api-keys"
  | "enterprise"
  | "debug"
  | "about";

// ── Helpers ───────────────────────────────────────────────────────────────────

/** Compute current consecutive-day streak from a newest-first history array.
 *  Uses LOCAL day-index so a 1am-IST recording doesn't end up bucketed as
 *  "yesterday UTC" and break the streak. */
function localDayIdx(ms: number): number {
  const d = new Date(ms);
  const localMidnight = new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  return Math.floor(localMidnight / 86_400_000);
}

function computeStreak(items: HistoryItem[]): number {
  if (items.length === 0) return 0;
  const todayDay = localDayIdx(Date.now());
  const activeDays = new Set(items.map((h) => localDayIdx(h.timestamp_ms)));
  let streak = 0;
  let day = todayDay;
  // Allow today OR yesterday as the streak start (don't break if user hasn't recorded today yet)
  if (!activeDays.has(day) && !activeDays.has(day - 1)) return 0;
  if (!activeDays.has(day)) day = day - 1;
  while (activeDays.has(day)) {
    streak++;
    day--;
  }
  return streak;
}

/** Map a backend Recording to the simpler HistoryItem for display. */
function recordingToHistoryItem(r: Recording): HistoryItem {
  return {
    timestamp_ms:      r.timestamp_ms,
    polished:          r.polished,
    word_count:        r.word_count,
    recording_seconds: r.recording_seconds,
    model:             r.model_used,
    transcribe_ms:     r.transcribe_ms ?? 0,
    embed_ms:          r.embed_ms ?? 0,
    polish_ms:         r.polish_ms ?? 0,
    audio_id:          r.audio_id,
    edit_count:        r.edit_count,
  };
}

// ── App ───────────────────────────────────────────────────────────────────────

// ── Setup loader (shown while first-launch migration runs) ───────────────────

const SETUP_STEPS = [
  "Restoring account",
  "Syncing API keys",
  "Uploading history",
  "Uploading vocabulary and corrections",
  "Preparing server memory",
];

function SetupLoader({ status }: { status: ServerMigrationStatus | null }) {
  const counts = status
    ? [
        status.uploaded_credentials_count,
        status.uploaded_credentials_count,
        status.uploaded_history_count,
        status.uploaded_vocab_count + status.uploaded_alias_count + status.uploaded_email_count,
        status.uploaded_vocab_count + status.uploaded_alias_count,
      ]
    : SETUP_STEPS.map(() => 0);

  const isRunning = !status || status.status === "running";

  return (
    <div className="flex h-screen w-screen items-center justify-center bg-[#0f0f10]">
      <div className="w-[340px] space-y-6">
        <div className="space-y-1">
          <p className="text-[15px] font-semibold text-white">Setting up your AirNote workspace</p>
          <p className="text-[12px] text-white/40">This only happens once and runs in the background.</p>
        </div>
        <div className="space-y-3">
          {SETUP_STEPS.map((step, i) => {
            const count = counts[i] ?? 0;
            const done = !isRunning || (status && count > 0);
            return (
              <div key={step} className="flex items-center gap-3">
                <div className={`w-4 h-4 rounded-full shrink-0 flex items-center justify-center text-[9px] font-bold ${done ? "bg-emerald-500/20 text-emerald-400" : "bg-white/10 text-white/30"}`}>
                  {done ? "✓" : String(i + 1)}
                </div>
                <span className={`text-[12px] ${done ? "text-white/70" : "text-white/30"}`}>{step}</span>
                {count > 0 && <span className="text-[11px] text-white/30 tabular-nums ml-auto">{count}</span>}
              </div>
            );
          })}
        </div>
        {status?.status === "failed" && (
          <p className="text-[11px] text-amber-400/70 pt-1">
            Some data could not be uploaded. You can retry later in Settings.
          </p>
        )}
      </div>
    </div>
  );
}

export default function App() {
  const [snapshot,    setSnapshot]    = useState<AppSnapshot | null>(null);
  const [history,     setHistory]     = useState<HistoryItem[]>([]);
  const [statusPhase, setStatusPhase] = useState<string>("");
  const [tokenBuf,    setTokenBuf]    = useState<string>("");
  const [busy,        setBusy]        = useState(false);
  const [errorBanner, setErrorBanner] = useState<string>("");
  const [activeView,  setActiveView]  = useState<ActiveView>("dashboard");
  const [liveMeetingId, setLiveMeetingId] = useState<string | null>(null);
  // When a live meeting ends we navigate to the Meetings page and focus the
  // just-ended meeting so its post-processing (transcribe → clean → summarize)
  // is shown there. This is the single post-meeting surface — LiveMeetingView no
  // longer renders its own duplicate "ended" notes layout.
  const [focusMeetingId, setFocusMeetingId] = useState<string | null>(null);
  const [inviteOpen,  setInviteOpen]  = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [settingsSection, setSettingsSection] = useState<SettingsSectionId>("models");
  const [performanceMonitorEnabled, setPerformanceMonitorEnabled] = useState(() => {
    try {
      return localStorage.getItem("said:performance-monitor-enabled") === "true";
    } catch {
      return false;
    }
  });
  const [onboardingComplete, setOnboardingComplete] = useState(() => {
    try {
      return localStorage.getItem("said:onboarding-complete") === "true";
    } catch {
      return false;
    }
  });
  // ── Retry toast ───────────────────────────────────────────────────────────
  const [retryToast, setRetryToast] = useState<{ message: string; audioId: string } | null>(null);

  // ── Edit confirmation toast ────────────────────────────────────────────────
  const [editToast, setEditToast] = useState<{
    recordingId: string; aiOutput: string; userKept: string;
  } | null>(null);

  // ── Vocabulary toast (manual add, auto-promote, star) ─────────────────────
  const [vocabToast, setVocabToast] = useState<VocabToastPayload | null>(null);

  // ── Download success toast ────────────────────────────────────────────────
  const [downloadToast, setDownloadToast] = useState<{ path: string } | null>(null);

  // ── Crash-recovered dictation (re-transcribed on launch) ──────────────────
  const [recoveredText, setRecoveredText] = useState<string | null>(null);
  const [recoveredCopied, setRecoveredCopied] = useState(false);

  // ── Pending edits ─────────────────────────────────────────────────────────
  const [pendingEdits, setPendingEdits] = useState<PendingEdit[]>([]);

  // ── History refresh key — incremented after each dictation to trigger reload
  const [historyRefreshKey, setHistoryRefreshKey] = useState(0);

  // ── Enterprise workspace gate ───────────────────────────────────────────────
  type EnterpriseGateState = "required" | "connected";
  const [enterpriseGate, setEnterpriseGate] = useState<EnterpriseGateState>(() =>
    isConnected() ? "connected" : "required",
  );

  const [_notifPerm,      setNotifPerm]       = useState<NotifPermission>("unknown"); // eslint-disable-line @typescript-eslint/no-unused-vars

  // ── First-launch server migration setup loader ─────────────────────────────
  type SetupLoaderState = "idle" | "running" | "done";
  const [setupLoader, setSetupLoader] = useState<SetupLoaderState>("idle");
  const [setupStatus, setSetupStatus] = useState<ServerMigrationStatus | null>(null);

  // Theme (light/dark) — persisted in localStorage, applied to <html>
  const { theme, toggle: toggleTheme } = useTheme();

  // Backend watchdog heartbeat — detects unresponsive backend and shows recovery overlay
  const heartbeat = useBackendHeartbeat();

  useEffect(() => startDailyAutoUpdateCheck(), []);

  const setPerformanceMonitor = useCallback((enabled: boolean) => {
    setPerformanceMonitorEnabled(enabled);
    try {
      localStorage.setItem("said:performance-monitor-enabled", String(enabled));
    } catch {
      // Ignore storage failures; the current session state still updates.
    }
  }, []);

  // ── Fetch history from backend ─────────────────────────────────────────────
  const refreshHistory = useCallback(async () => {
    const recs = await listHistory(100);
    setHistory(recs.map(recordingToHistoryItem));
  }, []);

  const refreshSnapshot = useCallback(async () => {
    const next = await invoke("get_snapshot");
    setSnapshot(next);
    return next;
  }, []);

  const refreshPermissionsSoon = useCallback(() => {
    const delays = [500, 1500, 3000, 5000];
    for (const delay of delays) {
      setTimeout(() => {
        void refreshSnapshot().catch(() => {});
        void checkNotificationPermission().then(setNotifPerm).catch(() => {});
      }, delay);
    }
  }, [refreshSnapshot]);

  // ── Bootstrap + enterprise check ───────────────────────────────────────────
  useEffect(() => {
    invoke("bootstrap")
      .then((snap) => {
        setSnapshot(snap as AppSnapshot);
      })
      .catch((err: unknown) => {
        setErrorBanner(err instanceof Error ? err.message : String(err));
      });
    refreshHistory();
  }, [refreshHistory]);

  useEffect(() => {
    let alive = true;
    (async () => {
      let restored: EnterpriseConnection | null = null;
      if (!isConnected()) {
        restored = await restoreConnectionFromLocalBackend();
      }
      if (!isConnected() && !restored) {
        if (alive) setEnterpriseGate("required");
        return;
      }
      const status = await checkConnection();
      if (!alive) return;
      if (status === "connected") {
        setEnterpriseGate("connected");
        const conn = getConnection() ?? restored;
        if (conn) {
          void ensureDesktopRegistered(conn.serverUrl, conn.jwt);
          void syncServerSettings();
          void syncCredentialVault();
        }
      } else {
        setEnterpriseGate("required");
      }
    })();
    return () => {
      alive = false;
    };
  }, []);

  // ── Enterprise heartbeat (every 5 min while connected) ────────────────────
  useEffect(() => {
    if (enterpriseGate !== "connected") return;

    const tick = () => {
      const conn = getConnection();
      if (!conn?.serverUrl || !conn.jwt) return;
      void ensureDesktopRegistered(conn.serverUrl, conn.jwt);
      void syncServerSettings();
      void syncCredentialVault();
      void syncCompanyVocab(false);
      void uploadUserVocabSummary();
    };
    tick();
    const interval = setInterval(tick, 5 * 60 * 1000);
    return () => clearInterval(interval);
  }, [enterpriseGate]);

  const handleEnterpriseConnected = useCallback((conn: EnterpriseConnection) => {
    setEnterpriseGate("connected");
    void ensureDesktopRegistered(conn.serverUrl, conn.jwt);
    void syncServerSettings();
    void syncCredentialVault();
    void syncCompanyVocab(true);
    void uploadUserVocabSummary(true);
  }, []);

  // ── Trigger migration once after enterprise connects ──────────────────────
  useEffect(() => {
    if (enterpriseGate !== "connected") return;
    let alive = true;
    (async () => {
      const currentStatus = await getMigrationStatus();
      if (!alive) return;
      if (currentStatus?.status === "completed") {
        setSetupLoader("done");
        return;
      }
      if (!currentStatus?.signed_in) {
        setSetupLoader("done");
        return;
      }
      setSetupLoader("running");
      await runMigration();
      // Poll until done or failed
      for (let i = 0; i < 60; i++) {
        if (!alive) return;
        await new Promise((r) => setTimeout(r, 1500));
        const s = await getMigrationStatus();
        if (!alive) return;
        setSetupStatus(s);
        if (s?.status === "completed" || s?.status === "failed" || s?.status === "partial") {
          break;
        }
      }
      if (alive) setSetupLoader("done");
    })();
    return () => { alive = false; };
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [enterpriseGate]);

  const handleEnterpriseDisconnect = useCallback(() => {
    setSettingsOpen(false);
    setEnterpriseGate("required");
  }, []);

  // Push the control-plane URL + session token to Rust so the Ctrl hold-to-talk
  // Divo hotkey activates (and de-activates on disconnect). Re-runs whenever the
  // enterprise connection state changes.
  useEffect(() => {
    if (enterpriseGate === "connected") {
      const conn = getConnection();
      void divoSetCredentials(conn?.serverUrl ?? "", conn?.jwt ?? "");
    } else {
      void divoSetCredentials("", "");
    }
  }, [enterpriseGate]);

  useEffect(() => {
    let alive = true;
    checkNotificationPermission().then((p) => {
      if (alive) setNotifPerm(p);
    });
    return () => {
      alive = false;
    };
  }, []);


  const handleDownloadSuccess = useCallback((path: string) => {
    setDownloadToast({ path });
  }, []);

  // ── Real-time Tauri event subscriptions ────────────────────────────────────
  useEffect(() => {
    // State changes pushed from Rust (hotkey recording, processing, done)
    const unsubState  = onAppState((snap) => {
      setSnapshot(snap);
      setBusy(snap.state === "processing");
      if (snap.state === "idle") {
        setStatusPhase("");
        setTokenBuf("");
      }
    });

    // Voice pipeline status (transcribing / polishing)
    const unsubStatus = onVoiceStatus((phase) => {
      setStatusPhase(phase);
    });

    // Individual LLM tokens for live preview
    const unsubToken  = onVoiceToken((token) => {
      setTokenBuf((prev) => prev + token);
    });

    // Final done event — refresh history with the new recording
    const unsubDone   = onVoiceDone((_done) => {
      refreshHistory();
      setHistoryRefreshKey((k) => k + 1);
      setTokenBuf("");
      setStatusPhase("");
    });

    // Voice error → show retry toast
    const unsubError = onVoiceError((msg, audioId, errorCode) => {
      setRetryToast({ message: msg, audioId: audioId ?? "" });
      setBusy(false);
      setSnapshot((p) => (p ? { ...p, state: "idle" } : p));
      setStatusPhase("");
      setTokenBuf("");
      if (errorCode === "missing_api_keys") {
        setRetryToast(null);
        setErrorBanner("API keys required — open Settings to add them.");
        setSettingsSection("api-keys");
        setSettingsOpen(true);
      }
    });

    // Edit detected (legacy in-app toast — still fires as fallback)
    const unsubEdit = onEditDetected((payload) => {
      setEditToast({
        recordingId: payload.recording_id,
        aiOutput:    payload.ai_output,
        userKept:    payload.user_kept,
      });
    });

    // Pending edits changed → refresh list, only notify for genuinely new edits.
    // Track IDs we've already shown in this session to avoid duplicate OS banners.
    const notifiedIds = new Set<string>();
    const sessionStartMs = Date.now();
    const refreshPending = async () => {
      const r = await getPendingEdits();
      setPendingEdits(r.edits);
      // Only notify for edits created during this session that we haven't shown yet.
      // Edits from previous sessions (older than 30s before session start) are stale.
      const cutoff = sessionStartMs - 30_000;
      const fresh = r.edits.filter(
        (e) => !notifiedIds.has(e.id) && e.timestamp_ms > cutoff
      );
      if (fresh.length > 0) {
        const edit = fresh[0];
        notifiedIds.add(edit.id);
        const ai   = edit.ai_output.length > 50 ? edit.ai_output.slice(0, 50) + "…" : edit.ai_output;
        const kept = edit.user_kept.length  > 50 ? edit.user_kept.slice(0, 50)  + "…" : edit.user_kept;
        sendNotification(
          "AirNote noticed an edit — tap to review",
          `"${ai}"  →  "${kept}"`
        );
      }
    };
    refreshPending();
    const unsubPending = onPendingEditsChanged(refreshPending);

    // Vocabulary toast — fires on auto-promote during dictation,
    // manual add via the Vocabulary panel, and star toggles.
    const unsubVocabToast = onVocabToast(setVocabToast);

    // Crash recovery — a dictation lost to a previous crash was re-transcribed.
    const unsubRecovered = onDictationRecovered((text) => {
      setRecoveredCopied(false);
      setRecoveredText(text);
    });

    // Tray menu → navigate to Settings
    const unsubNav = onNavSettings(() => {
      setSettingsSection("models");
      setSettingsOpen(true);
    });

    return () => {
      unsubNav();
      unsubState();
      unsubStatus();
      unsubToken();
      unsubDone();
      unsubError();
      unsubEdit();
      unsubPending();
      unsubVocabToast();
      unsubRecovered();
    };
  }, [refreshHistory]);

  // ── Periodic snapshot poll — picks up Accessibility/Input Monitoring grants ──
  // 5 s is fast enough — permission changes require a user trip to System Settings.
  useEffect(() => {
    const interval = setInterval(async () => {
      if (busy) return;
      try {
        await refreshSnapshot();
      } catch {
        // silently ignore
      }
    }, 5000);
    return () => clearInterval(interval);
  }, [busy, refreshSnapshot]);

  useEffect(() => {
    const refresh = () => {
      if (busy) return;
      void refreshSnapshot().catch(() => {});
      void checkNotificationPermission().then(setNotifPerm).catch(() => {});
    };
    window.addEventListener("focus", refresh);
    document.addEventListener("visibilitychange", refresh);
    return () => {
      window.removeEventListener("focus", refresh);
      document.removeEventListener("visibilitychange", refresh);
    };
  }, [busy, refreshSnapshot]);

  // ── Record toggle (button click) ───────────────────────────────────────────
  const handleToggle = useCallback(async () => {
    if (!snapshot) return;
    setErrorBanner("");
    if (snapshot.state === "recording") {
      setBusy(true);
      setSnapshot((p) => (p ? { ...p, state: "processing" } : p));
    }
    try {
      const next = await invoke("toggle_recording");
      setSnapshot(next);
      if (next.state === "idle") {
        await refreshHistory();
        setBusy(false);
      }
    } catch (err: unknown) {
      setErrorBanner(err instanceof Error ? err.message : String(err));
      setSnapshot((p) => (p ? { ...p, state: "idle" } : p));
      setBusy(false);
    }
  }, [snapshot, refreshHistory]);

  // ── Accessibility ──────────────────────────────────────────────────────────
  const handleAccessibility = useCallback(async () => {
    setErrorBanner("");
    try {
      const next = await invoke("request_accessibility");
      setSnapshot(next);
      refreshPermissionsSoon();
    } catch (err: unknown) {
      setErrorBanner(err instanceof Error ? err.message : String(err));
    }
  }, [refreshPermissionsSoon]);

  const handleMicrophone = useCallback(async () => {
    setErrorBanner("");
    try {
      const next = await requestMicrophone();
      setSnapshot(next);
      refreshPermissionsSoon();
    } catch (err: unknown) {
      setErrorBanner(err instanceof Error ? err.message : String(err));
    }
  }, [refreshPermissionsSoon]);

  // ── Input Monitoring ───────────────────────────────────────────────────────
  const handleInputMonitoring = useCallback(async () => {
    setErrorBanner("");
    try {
      await requestInputMonitoring();
      // Re-read snapshot after a short delay to pick up new permission state
      setTimeout(async () => {
        try {
          const next = await invoke("get_snapshot");
          setSnapshot(next);
        } catch { /* ignore */ }
      }, 1000);
      refreshPermissionsSoon();
    } catch (err: unknown) {
      setErrorBanner(err instanceof Error ? err.message : String(err));
    }
  }, [refreshPermissionsSoon]);

  const handleOnboardingFinish = useCallback(() => {
    setOnboardingComplete(true);
    try {
      localStorage.setItem("said:onboarding-complete", "true");
    } catch { /* ignore */ }
  }, []);

  // ── Navigation ─────────────────────────────────────────────────────────────
  const handleViewChange = useCallback((view: string) => {
    // Settings is now a modal — intercept the route and open the modal instead
    if (view === "settings") {
      setSettingsSection("models");
      setSettingsOpen(true);
      return;
    }
    if (VALID_VIEWS.includes(view as ActiveView)) {
      setActiveView(view as ActiveView);
      // Refresh history when user opens the history tab
      if (view === "history") refreshHistory();
    }
  }, [refreshHistory]);

  // ── Merge history into snapshot for child components ──────────────────────
  const snapshotWithHistory: AppSnapshot | null = snapshot
    ? {
        ...snapshot,
        history,
        total_words:  history.reduce((s, h) => s + h.word_count, 0),
        daily_streak: computeStreak(history),
        avg_wpm:      (() => {
          const recent = history.slice(0, 10);
          if (!recent.length) return 0;
          const tw = recent.reduce((s, h) => s + h.word_count, 0);
          const tm = recent.reduce((s, h) => s + h.recording_seconds / 60, 0);
          const raw = tm > 0 ? Math.round(tw / tm) : 0;
          return Math.min(raw, 300);
        })(),
      }
    : null;

  // ── Live status / token overlay for DashboardView ─────────────────────────
  // We pass these as extra props; DashboardView can render a streaming preview.
  const liveText = statusPhase === "polishing" ? tokenBuf : "";
  const corePermissionsReady =
    !!snapshot?.microphone_granted &&
    !!snapshot?.accessibility_granted &&
    !!snapshot?.input_monitoring_granted;

  const needsEnterprise = enterpriseGate === "required";
  const needsSetup = !corePermissionsReady || !onboardingComplete;
  const workspaceOnly = needsEnterprise && onboardingComplete && corePermissionsReady;

  if (needsEnterprise || needsSetup) {
    return (
      <OnboardingFlow
        snapshot={snapshotWithHistory}
        workspaceOnly={workspaceOnly}
        enterpriseRequired={needsEnterprise}
        onEnterpriseConnected={handleEnterpriseConnected}
        onMicrophone={handleMicrophone}
        onAccessibility={handleAccessibility}
        onInputMonitoring={handleInputMonitoring}
        onFinish={handleOnboardingFinish}
      />
    );
  }

  if (setupLoader === "running") {
    return <SetupLoader status={setupStatus} />;
  }

  const liveMeetingActive = activeView === "live-meeting" && !!liveMeetingId;

  /* ── Render ─────────────────────────────────────────────────────────────── */
  return (
    <div className="flex h-screen w-screen overflow-hidden">

      {liveMeetingActive ? (
        <div className="min-w-0 flex-1">
          <LiveMeetingView
            meetingId={liveMeetingId}
            onBack={() => setActiveView("meetings")}
            onEnded={(id) => {
              // Hand the just-ended meeting to the Meetings page and switch
              // to it. Processing continues in the background; the Meetings
              // detail polls and renders the live stage progress.
              setFocusMeetingId(id);
              setActiveView("meetings");
            }}
          />
        </div>
      ) : (
        <>
          {/* ── Sidebar — full height left column ────────── */}
          <Sidebar
            snapshot={snapshotWithHistory}
            activeView={activeView}
            onViewChange={handleViewChange}
            busy={busy}
            performanceMonitorEnabled={performanceMonitorEnabled}
            onOpenInvite={() => setInviteOpen(true)}
          />

          {/* ── Right column: topbar + content ───────────── */}
          <div className="flex flex-col flex-1 overflow-hidden min-w-0">

            <Topbar
              snapshot={snapshotWithHistory}
              theme={theme}
              toggleTheme={toggleTheme}
              onEnterpriseDisconnect={handleEnterpriseDisconnect}
            />

            {/* ── The "mat" — elevated content surface ───────
                Dense near-black, mostly opaque so the content area reads as
                solid. Values tuned via the live glass control panel at
                .context/said-glass-control.html. */}
            <main className="flex-1 overflow-hidden p-3 pt-2">
              <div
                className="h-full rounded-2xl overflow-hidden"
                style={{
                  background: "hsl(var(--glass-bg-strong))",
                  backdropFilter: "blur(40px) saturate(190%)",
                  WebkitBackdropFilter: "blur(40px) saturate(190%)",
                  boxShadow: "var(--shadow-glass)",
                }}
              >
                {activeView === "dashboard" && (
                  <DashboardView
                    snapshot={snapshotWithHistory}
                    busy={busy}
                    onToggle={handleToggle}
                    onAccessibility={handleAccessibility}
                    onNavigate={handleViewChange}
                    statusPhase={statusPhase}
                    liveText={liveText}
                    pendingEdits={pendingEdits}
                    onDownloadSuccess={handleDownloadSuccess}
                    refreshKey={historyRefreshKey}
                    onResolvePending={async (id, action) => {
                      await resolvePendingEdit(id, action);
                      setPendingEdits((prev) => prev.filter((e) => e.id !== id));
                    }}
                  />
                )}
                {activeView === "history"    && <HistoryView onDownloadSuccess={handleDownloadSuccess} refreshKey={historyRefreshKey} />}
                {activeView === "vocabulary" && <VocabularyView />}
                {activeView === "insights"   && <InsightsView snapshot={snapshotWithHistory} />}
                {activeView === "meetings"   && (
                  <MeetingsView
                    focusMeetingId={focusMeetingId}
                    onFocusConsumed={() => setFocusMeetingId(null)}
                    onConfigureModels={() => {
                      setSettingsSection("meeting");
                      setSettingsOpen(true);
                    }}
                    onOpenWorkspaces={() => {
                      setSettingsSection("enterprise");
                      setSettingsOpen(true);
                    }}
                    onJoinMeeting={(id) => {
                      setLiveMeetingId(id);
                      setActiveView("live-meeting");
                    }}
                  />
                )}
                {activeView === "divo" && <DivoView />}
                {/* Settings is now a modal — opened via setSettingsOpen */}
              </div>
            </main>
          </div>
        </>
      )}

      {/* ── Invite team modal (overlays everything) ────── */}
      <InviteTeamModal open={inviteOpen} onClose={() => setInviteOpen(false)} />

      {/* ── Settings modal (replaces the old Settings route) ── */}
      <SettingsModal
        open={settingsOpen}
        onClose={() => setSettingsOpen(false)}
        snapshot={snapshotWithHistory}
        onAccessibility={handleAccessibility}
        onInputMonitoring={handleInputMonitoring}
        onMicrophone={handleMicrophone}
        performanceMonitorEnabled={performanceMonitorEnabled}
        onPerformanceMonitorChange={setPerformanceMonitor}
        onEnterpriseDisconnect={handleEnterpriseDisconnect}
        initialSection={settingsSection}
      />

      {/* ── Retry toast (bottom-center) ──────────────── */}
      {retryToast && (
        <RetryToast
          message={retryToast.message}
          canRetry={retryToast.audioId.length > 0}
          onRetry={async () => {
            setRetryToast(null);
            if (retryToast.audioId) {
              try {
                await invoke("retry_recording", { audioId: retryToast.audioId });
              } catch (e) {
                setErrorBanner(e instanceof Error ? e.message : String(e));
              }
            }
          }}
          onOpenHistory={() => {
            setRetryToast(null);
            handleViewChange("history");
          }}
          onDismiss={() => setRetryToast(null)}
        />
      )}

      {/* ── Edit confirmation toast (bottom-center) ── */}
      {editToast && !retryToast && (
        <EditConfirmToast
          aiOutput={editToast.aiOutput}
          userKept={editToast.userKept}
          onSave={async () => {
            setEditToast(null);
            try {
              await submitEditFeedback(editToast.recordingId, editToast.userKept);
            } catch { /* non-critical */ }
          }}
          onDismiss={() => setEditToast(null)}
        />
      )}

      {/* ── Vocabulary toast (bottom-center) ─────────── */}
      {vocabToast && !retryToast && !editToast && (
        <VocabularyToast
          kind={vocabToast.kind}
          term={vocabToast.term}
          source={vocabToast.source}
          onUndo={vocabToast.kind === "added" ? async () => {
            const t = vocabToast.term;
            setVocabToast(null);
            try {
              await deleteVocabularyTerm(t);
            } catch { /* non-critical */ }
          } : undefined}
          onDismiss={() => setVocabToast(null)}
        />
      )}

      {/* ── Download success toast (bottom-center) ─── */}
      {downloadToast && !retryToast && !editToast && !vocabToast && (
        <DownloadSuccessToast
          path={downloadToast.path}
          onReveal={() => {
            void revealDownloadedFile(downloadToast.path).catch((err) => {
              setErrorBanner(err instanceof Error ? err.message : String(err));
            });
          }}
          onDismiss={() => setDownloadToast(null)}
        />
      )}

      {/* ── Floating error toast ──────────────────────── */}
      {errorBanner && (
        <div
          className="fixed bottom-4 right-4 max-w-sm rounded-xl px-4 py-3 flex items-start gap-3 z-50"
          style={{
            background: "hsl(0 75% 60% / 0.12)",
            color:      "hsl(0 75% 80%)",
          }}
        >
          <p className="text-[13px] flex-1 leading-snug">{errorBanner}</p>
          <button
            onClick={() => setErrorBanner("")}
            className="flex-shrink-0 transition-colors mt-0.5 no-drag opacity-60 hover:opacity-100"
          >
            <X size={14} />
          </button>
        </div>
      )}

      {/* ── Recovered dictation (after a crash) ───────── */}
      {recoveredText && (
        <div
          className="fixed inset-0 z-[60] flex items-center justify-center p-6"
          style={{ background: "hsl(0 0% 0% / 0.55)" }}
        >
          <div
            className="w-full max-w-lg rounded-2xl p-5 flex flex-col gap-4"
            style={{
              background: "hsl(240 10% 8% / 0.98)",
              border: "1px solid hsl(240 8% 24%)",
              boxShadow: "0 24px 64px hsl(0 0% 0% / 0.5)",
            }}
          >
            <div className="flex items-start justify-between gap-3">
              <div>
                <h2 className="text-[15px] font-semibold" style={{ color: "hsl(240 10% 96%)" }}>
                  Recovered your last dictation
                </h2>
                <p className="text-[12px] mt-1" style={{ color: "hsl(240 6% 64%)" }}>
                  AirNote closed unexpectedly while you were speaking. Here's what you said:
                </p>
              </div>
              <button
                onClick={() => setRecoveredText(null)}
                className="flex-shrink-0 transition-colors opacity-60 hover:opacity-100"
                style={{ color: "hsl(240 10% 96%)" }}
                aria-label="Dismiss"
              >
                <X size={16} />
              </button>
            </div>
            <div
              className="rounded-xl px-4 py-3 text-[13px] leading-relaxed max-h-64 overflow-y-auto whitespace-pre-wrap"
              style={{ background: "hsl(240 10% 12%)", color: "hsl(240 10% 92%)" }}
            >
              {recoveredText}
            </div>
            <div className="flex items-center justify-end gap-2">
              <button
                onClick={() => setRecoveredText(null)}
                className="rounded-lg px-3 py-2 text-[13px] no-drag transition-colors"
                style={{ color: "hsl(240 6% 70%)" }}
              >
                Dismiss
              </button>
              <button
                onClick={() => {
                  navigator.clipboard
                    .writeText(recoveredText)
                    .then(() => setRecoveredCopied(true))
                    .catch((err) => setErrorBanner(err instanceof Error ? err.message : String(err)));
                }}
                className="rounded-lg px-4 py-2 text-[13px] font-medium no-drag transition-colors"
                style={{ background: "hsl(255 80% 62%)", color: "white" }}
              >
                {recoveredCopied ? "Copied ✓" : "Copy text"}
              </button>
            </div>
          </div>
        </div>
      )}

      {/* ── Watchdog reconnecting overlay ────────────── */}
      <ReconnectingOverlay
        level={heartbeat.level}
        showOverlay={heartbeat.showOverlay}
        justRecovered={heartbeat.justRecovered}
      />
    </div>
  );
}
