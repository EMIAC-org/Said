import React, { useEffect, useState, useCallback } from "react";
import { X, AlertCircle, Loader2, ArrowRight } from "lucide-react";
import { Sidebar } from "@/components/Sidebar";
import { BrandMark } from "@/components/BrandMark";
import { InviteTeamModal } from "@/components/InviteTeamModal";
import { SettingsModal } from "@/components/SettingsModal";
import { OnboardingFlow } from "@/components/OnboardingFlow";
import { Topbar } from "@/components/Topbar";
import { DashboardView } from "@/components/views/DashboardView";
import { HistoryView } from "@/components/views/HistoryView";
import { InsightsView } from "@/components/views/InsightsView";
import { VocabularyView } from "@/components/views/VocabularyView";
import { MeetingsView } from "@/components/views/MeetingsView";
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
  cloudLogin,
  cloudSignup,
  requestInputMonitoring,
  requestMicrophone,
  submitEditFeedback,
  onVocabToast,
  deleteVocabularyTerm,
  checkNotificationPermission,
  revealDownloadedFile,
  type NotifPermission,
  type VocabToastPayload,
} from "@/lib/invoke";
import { useTheme } from "@/lib/useTheme";
import { useBackendHeartbeat } from "@/lib/useBackendHeartbeat";
import { ReconnectingOverlay } from "@/components/ReconnectingOverlay";
import type { AppSnapshot, HistoryItem, PendingEdit, Recording } from "@/types";
import { RetryToast, EditConfirmToast, VocabularyToast, DownloadSuccessToast } from "@/components/NotificationToast";

export type ActiveView = "dashboard" | "history" | "vocabulary" | "insights" | "meetings" | "settings" | "live-meeting";
const VALID_VIEWS: ActiveView[] = ["dashboard", "history", "vocabulary", "insights", "meetings", "settings", "live-meeting"];
type SettingsSectionId = "writing" | "permissions" | "api-keys" | "debug" | "about";

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

export default function App() {
  const [snapshot,    setSnapshot]    = useState<AppSnapshot | null>(null);
  const [history,     setHistory]     = useState<HistoryItem[]>([]);
  const [statusPhase, setStatusPhase] = useState<string>("");
  const [tokenBuf,    setTokenBuf]    = useState<string>("");
  const [busy,        setBusy]        = useState(false);
  const [errorBanner, setErrorBanner] = useState<string>("");
  const [activeView,  setActiveView]  = useState<ActiveView>("dashboard");
  const [liveMeetingId, setLiveMeetingId] = useState<string | null>(null);
  const [inviteOpen,  setInviteOpen]  = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [settingsSection, setSettingsSection] = useState<SettingsSectionId>("writing");
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

  // ── Pending edits ─────────────────────────────────────────────────────────
  const [pendingEdits, setPendingEdits] = useState<PendingEdit[]>([]);

  // ── History refresh key — incremented after each dictation to trigger reload
  const [historyRefreshKey, setHistoryRefreshKey] = useState(0);

  // ── Cloud auth gate ────────────────────────────────────────────────────────
  // null = still checking, false = signed in, true = needs sign-in
  const [needsAuth,   setNeedsAuth]   = useState<boolean | null>(null);
  const [authMode,    setAuthMode]    = useState<"login" | "signup">("login");
  const [authEmail,   setAuthEmail]   = useState("");
  const [authPass,    setAuthPass]    = useState("");
  const [authBusy,    setAuthBusy]    = useState(false);
  const [authError,   setAuthError]   = useState("");

  const [_notifPerm,      setNotifPerm]       = useState<NotifPermission>("unknown"); // eslint-disable-line @typescript-eslint/no-unused-vars

  // Theme (light/dark) — persisted in localStorage, applied to <html>
  const { theme, toggle: toggleTheme } = useTheme();

  // Backend watchdog heartbeat — detects unresponsive backend and shows recovery overlay
  const heartbeat = useBackendHeartbeat();

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

  // ── Bootstrap + auth check ─────────────────────────────────────────────────
  useEffect(() => {
    invoke("bootstrap")
      .then((snap) => {
        setSnapshot(snap as AppSnapshot);
        setNeedsAuth(false);
      })
      .catch((err: unknown) => {
        setErrorBanner(err instanceof Error ? err.message : String(err));
        setNeedsAuth(false);
      });
    refreshHistory();
  }, [refreshHistory]);

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

  // ── Auth submit ────────────────────────────────────────────────────────────
  const handleAuthSubmit = useCallback(async (e: React.FormEvent) => {
    e.preventDefault();
    setAuthBusy(true);
    setAuthError("");
    try {
      if (authMode === "login") {
        await cloudLogin(authEmail, authPass);
      } else {
        await cloudSignup(authEmail, authPass);
      }
      setNeedsAuth(false);
      refreshHistory();
    } catch (err: unknown) {
      setAuthError(err instanceof Error ? err.message : String(err));
    } finally {
      setAuthBusy(false);
    }
  }, [authMode, authEmail, authPass, refreshHistory]);

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

    // Tray menu → navigate to Settings
    const unsubNav = onNavSettings(() => {
      setSettingsSection("writing");
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
      setSettingsSection("writing");
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

  /* ── Auth gate ──────────────────────────────────────────────────────────── */
  if (needsAuth === null) {
    // Still checking — bare loading splash with the same brand as the auth screens
    return (
      <div
        data-tauri-drag-region
        className="flex h-screen w-screen items-center justify-center"
        style={{ background: "hsl(var(--background))" }}
      >
        <div className="flex flex-col items-center gap-3">
          <BrandMark size={36} idSuffix="loading" className="opacity-70" />
          <span className="text-[12px] text-muted-foreground">Starting AirNote…</span>
        </div>
      </div>
    );
  }

  if (!corePermissionsReady || !onboardingComplete) {
    return (
      <OnboardingFlow
        snapshot={snapshotWithHistory}
        onMicrophone={handleMicrophone}
        onAccessibility={handleAccessibility}
        onInputMonitoring={handleInputMonitoring}
        onFinish={handleOnboardingFinish}
      />
    );
  }

  if (needsAuth) {
    return (
      <div
        className="flex h-screen w-screen items-center justify-center relative overflow-hidden"
        style={{ background: "hsl(var(--background))" }}
      >
        <div aria-hidden data-tauri-drag-region className="absolute inset-x-0 top-0 h-12 drag-region" />

        {/* Mint hero glow — same wash used on the dashboard + invite modal */}
        <div
          aria-hidden
          className="absolute pointer-events-none"
          style={{
            top: "-15%", left: "50%", transform: "translateX(-50%)",
            width: 640, height: 640, borderRadius: "50%",
            background: "radial-gradient(circle, hsl(var(--primary) / 0.10) 0%, transparent 65%)",
          }}
        />

        <div
          className="relative w-full max-w-[340px] flex flex-col p-7 rounded-[18px]"
          style={{
            background: "hsl(var(--surface-2))",
            boxShadow: "var(--shadow-glass)",
          }}
        >

          {/* Brand — tight stack */}
          <div className="flex flex-col items-center gap-2.5 mb-5">
            <BrandMark size={40} idSuffix="auth-login" />
            <div className="text-center">
              <h1
                className="text-[18px] font-extrabold tracking-tight"
                style={{ color: "hsl(var(--foreground))", letterSpacing: "-0.02em" }}
              >
                {authMode === "login" ? "Welcome back" : "Welcome to AirNote"}
              </h1>
              <p className="text-[11.5px] text-muted-foreground mt-1">
                {authMode === "login"
                  ? "Sign in to sync your vocabulary."
                  : "Free while we're early."}
              </p>
            </div>
          </div>

          {/* Mode toggle — same pill pattern, tighter */}
          <div
            className="flex gap-1 p-0.5 rounded-lg mb-3.5"
            style={{ background: "hsl(var(--surface-1))" }}
          >
            {(["login", "signup"] as const).map((m) => {
              const isActive = authMode === m;
              return (
                <button
                  key={m}
                  onClick={() => { setAuthMode(m); setAuthError(""); }}
                  className="flex-1 py-1 text-[11.5px] font-semibold rounded-md transition-all"
                  style={{
                    background: isActive ? "hsl(var(--pill-active-bg))" : "transparent",
                    color:      isActive ? "hsl(var(--pill-active-fg))" : "hsl(var(--muted-foreground))",
                  }}
                >
                  {m === "login" ? "Sign in" : "Create account"}
                </button>
              );
            })}
          </div>

          {/* Form — shared .input class, tighter spacing */}
          <form onSubmit={handleAuthSubmit} className="flex flex-col gap-2">
            <input
              type="email"
              placeholder="you@example.com"
              autoComplete="email"
              value={authEmail}
              onChange={(e) => setAuthEmail(e.target.value)}
              required
              className="input"
              style={{ fontSize: 13 }}
            />
            <input
              type="password"
              placeholder="Password"
              autoComplete={authMode === "login" ? "current-password" : "new-password"}
              value={authPass}
              onChange={(e) => setAuthPass(e.target.value)}
              required
              className="input"
              style={{ fontSize: 13 }}
            />

            {authError && (
              <div
                className="flex items-center gap-1.5 px-2.5 py-1.5 rounded-md mt-0.5"
                style={{
                  background: "hsl(354 78% 60% / 0.10)",
                  color:      "hsl(354 78% 75%)",
                  boxShadow:  "inset 0 0 0 1px hsl(354 78% 60% / 0.25)",
                }}
              >
                <AlertCircle size={12} className="flex-shrink-0" />
                <span className="text-[11.5px] font-medium">{authError}</span>
              </div>
            )}

            {/* Primary CTA — same .btn-primary, tighter padding */}
            <button
              type="submit"
              disabled={authBusy || !authEmail || !authPass}
              className="btn-primary mt-2 w-full justify-center py-2 rounded-lg"
              style={{ fontSize: 12.5 }}
            >
              {authBusy ? (
                <>
                  <Loader2 size={13} className="animate-spin" />
                  {authMode === "login" ? "Signing in…" : "Creating account…"}
                </>
              ) : (
                <>
                  {authMode === "login" ? "Sign in" : "Create account"}
                  <ArrowRight size={12} />
                </>
              )}
            </button>
          </form>

          {/* Offline escape — quiet, single line */}
          <button
            onClick={() => setNeedsAuth(false)}
            className="text-[11px] text-muted-foreground hover:text-foreground text-center transition-colors mt-4"
          >
            Continue without an account
          </button>
        </div>
      </div>
    );
  }

  /* ── Render ─────────────────────────────────────────────────────────────── */
  return (
    <div className="flex h-screen w-screen overflow-hidden">

      {/* ── Sidebar — full height left column ────────── */}
      <Sidebar
        snapshot={snapshotWithHistory}
        activeView={activeView}
        onViewChange={handleViewChange}
        busy={busy}
        performanceMonitorEnabled={performanceMonitorEnabled}
        onOpenInvite={() => setInviteOpen(true)}
      />

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

        initialSection={settingsSection}
      />

      {/* ── Right column: topbar + content ───────────── */}
      <div className="flex flex-col flex-1 overflow-hidden min-w-0">

        <Topbar
          snapshot={snapshotWithHistory}
          theme={theme}
          toggleTheme={toggleTheme}
          onLoginClick={() => setNeedsAuth(true)}
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
                onJoinMeeting={(id) => {
                  setLiveMeetingId(id);
                  setActiveView("live-meeting");
                }}
              />
            )}
            {activeView === "live-meeting" && liveMeetingId && (
              <LiveMeetingView
                meetingId={liveMeetingId}
                onBack={() => setActiveView("meetings")}
              />
            )}
            {/* Settings is now a modal — opened via setSettingsOpen */}
          </div>
        </main>
      </div>

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

      {/* ── Watchdog reconnecting overlay ────────────── */}
      <ReconnectingOverlay
        level={heartbeat.level}
        showOverlay={heartbeat.showOverlay}
        justRecovered={heartbeat.justRecovered}
      />
    </div>
  );
}
