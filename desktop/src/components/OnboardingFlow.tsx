import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  ArrowRight,
  Check,
  Cpu,
  Download,
  Mic,
  Shield,
  Keyboard,
  Link,
  Wifi,
  LogOut,
  Loader2,
  Sparkles,
  ExternalLink,
  X,
} from "lucide-react";
import { listen } from "@tauri-apps/api/event";
import { OnboardingShell } from "@/components/OnboardingShell";
import { EnterpriseConnectForm } from "@/components/EnterpriseConnectForm";
import type { EnterpriseConnection } from "@/lib/enterprise";
import {
  getConnection,
  completeEmailAuth,
  DEFAULT_CLOUD_SERVER_URL,
  loadSavedAuthMode,
} from "@/lib/enterprise";
import type { AppSnapshot, Preferences } from "@/types";
import {
  getPreferences, invoke, patchPreferences,
  getDesktopPrefs, setDesktopPrefs, requestBrowserAutomation,
} from "@/lib/invoke";
import { NEW_MODEL_FILE, NEW_MODEL_NAME, NEW_MODEL_SIZE_HINT } from "@/lib/onDeviceModel";
import { ReclaimOldModelsRow, type ReclaimResult } from "@/components/ReclaimOldModelsRow";
import { friendlyError } from "@/lib/friendlyError";
import { ErrorNotice } from "./ErrorNotice";
import { HotkeyPicker } from "@/components/HotkeyPicker";
import { hotkeyDisplay, hotkeyMode, type Platform } from "@/lib/hotkeys";
import {
  clearOnboardingProgress,
  computeResumeProgress,
  loadOnboardingProgress,
  ONBOARDING_STEPS,
  ONBOARDING_STEP_IDS,
  saveOnboardingProgress,
  shellStepStatus,
  firstUndoneStep,
  type OnboardingProgress,
  type OnboardingStep,
} from "@/lib/onboardingProgress";

type Step = OnboardingStep;

interface Props {
  snapshot: AppSnapshot | null;
  onMicrophone: () => void;
  onAccessibility: () => void;
  onInputMonitoring: () => void;
  onFinish: () => void;
  /** Re-read the permission snapshot (used to poll while on the permissions step
   *  so a grant made in System Settings is picked up without a window refocus). */
  onRefreshPermissions?: () => void;
  /** When true, user must connect workspace before continuing setup. */
  enterpriseRequired?: boolean;
  /** Called when workspace OAuth completes. */
  onEnterpriseConnected?: (conn: EnterpriseConnection) => void;
  /** Reconnect-only mode — skip welcome/permissions/keys/hotkey. */
  workspaceOnly?: boolean;
  /** Restored onboarding progress from localStorage. */
  initialProgress?: OnboardingProgress | null;
  /** Existing users may be forced back here until the local dictation model is installed. */
  requireLocalModelSetup?: boolean;
  /** Called once the local dictation model is installed. */
  onLocalModelReady?: () => void;
}

interface DictationModelStatus {
  installed: boolean;
  size_bytes: number;
  path: string;
}

// Payload of the shared `meeting-model-download` event (keyed by model name).
interface DictationDownloadProgress {
  name: string;
  received: number;
  total: number;
  status: "downloading" | "done" | "cancelled" | "error";
  error: string | null;
}

function formatSize(bytes: number): string {
  if (bytes >= 1e9) return `${(bytes / 1e9).toFixed(1)} GB`;
  if (bytes >= 1e6) return `${Math.round(bytes / 1e6)} MB`;
  if (bytes > 0) return `${Math.round(bytes / 1e3)} KB`;
  return "—";
}

const STEPS = ONBOARDING_STEP_IDS;

// All desktop platforms get the same high-level onboarding order. The speech
// recognition step renders platform-specific copy but always includes the
// cross-platform whisper.cpp local model download.
function visibleStepsFor(): Step[] {
  return [...STEPS];
}


// ── Main component ─────────────────────────────────────────────────────────

export function OnboardingFlow({
  snapshot,
  onMicrophone,
  onAccessibility,
  onInputMonitoring,
  onFinish,
  onRefreshPermissions,
  enterpriseRequired = false,
  onEnterpriseConnected,
  workspaceOnly = false,
  initialProgress = null,
  requireLocalModelSetup = false,
  onLocalModelReady,
}: Props) {
  const [prefs, setPrefs] = useState<Preferences | null>(null);
  const [keySaving, setKeySaving] = useState(false);
  const [keyError, setKeyError] = useState("");
  const [dictationTried, setDictationTried] = useState(false);
  // Mirror of dictationTried readable inside timers/listeners without re-subscribing.
  const dictationTriedRef = useRef(false);
  const [dictationModel, setDictationModel] = useState<DictationModelStatus | null>(null);
  const [dictationDownload, setDictationDownload] = useState<DictationDownloadProgress | null>(null);
  const [dictationBusy, setDictationBusy] = useState(false);
  const [dictationError, setDictationError] = useState("");
  // Old-model cleanup (only offered once the new model is verified installed).
  const [reclaiming, setReclaiming] = useState(false);
  const [reclaimResult, setReclaimResult] = useState<ReclaimResult | null>(null);
  const [reclaimError, setReclaimError] = useState("");
  // Live "Try it" feedback so a failed first dictation is never a silent empty box.
  const [testError, setTestError] = useState("");
  const [testPhase, setTestPhase] = useState<"idle" | "recording" | "processing">("idle");
  const [testNoAudio, setTestNoAudio] = useState(false);
  const [workspacePreview, setWorkspacePreview] = useState<EnterpriseConnection | null>(null);
  const [userNavigatedManually, setUserNavigatedManually] = useState(false);
  const resumeSynced = useRef(false);
  const silentLegacyModelCleanupDone = useRef(false);

  const [progress, setProgress] = useState<OnboardingProgress>(() =>
    computeResumeProgress(initialProgress ?? loadOnboardingProgress(), {
      workspaceOnly,
      snapshot: null,
    }),
  );

  const [authMode, setAuthMode] = useState<"personal" | "workspace">(() => {
    if (workspaceOnly) {
      return loadSavedAuthMode() ?? initialProgress?.authMode ?? progress.authMode;
    }
    return progress.authMode;
  });

  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [emailSignup, setEmailSignup] = useState(true);
  const [personalLoading, setPersonalLoading] = useState(false);
  const [personalError, setPersonalError] = useState("");

  const micGranted = snapshot?.microphone_granted ?? false;
  const accGranted = snapshot?.accessibility_granted ?? false;
  const imGranted = snapshot?.input_monitoring_granted ?? false;
  const isWindows = snapshot?.platform === "windows";
  const isMac = snapshot?.platform === "macos";
  const hkPlatform: Platform = isWindows ? "windows" : "macos";

  const [step, setStep] = useState<Step>(() => progress.currentStep);

  const visibleStepIds = useMemo(() => visibleStepsFor(), []);
  const totalSteps = visibleStepIds.length;
  const visStepIndex = useCallback(
    (s: Step) => {
      const i = visibleStepIds.indexOf(s);
      return i >= 0 ? i : 0;
    },
    [visibleStepIds],
  );
  const stepLabel = useCallback(
    (s: Step) => `${visStepIndex(s) + 1} of ${totalSteps}`,
    [visStepIndex, totalSteps],
  );

  useEffect(() => {
    getPreferences().then((p) => {
      if (p) {
        setPrefs(p);
      }
    });
  }, []);

  const refreshDictationModel = useCallback(async () => {
    try {
      const status = await invoke<DictationModelStatus>("dictation_model_status");
      setDictationModel(status);
      if (status.installed) onLocalModelReady?.();
      return status;
    } catch (e) {
      setDictationError(e instanceof Error ? e.message : String(e));
      return null;
    }
  }, [onLocalModelReady]);

  useEffect(() => {
    void refreshDictationModel();
  }, [refreshDictationModel]);

  useEffect(() => {
    if (silentLegacyModelCleanupDone.current || !dictationModel || dictationModel.installed) return;
    silentLegacyModelCleanupDone.current = true;
    void invoke<ReclaimResult>("reclaim_old_models")
      .then((result) => {
        if (result.removed.length > 0) void refreshDictationModel();
      })
      .catch(() => {});
  }, [dictationModel, refreshDictationModel]);

  useEffect(() => {
    const unlistenP = listen<DictationDownloadProgress>("meeting-model-download", (event) => {
      const payload = event.payload;
      // The event is shared across models (new model + VAD); only react to the
      // new model so VAD's silent auto-fetch never shows in onboarding.
      if (payload.name !== NEW_MODEL_FILE) return;
      if (payload.status === "downloading") {
        setDictationDownload(payload);
        setDictationError("");
      } else {
        setDictationDownload(null);
      }
      if (payload.status === "done") {
        void refreshDictationModel();
      }
      if (payload.status === "error") {
        setDictationError(friendlyError(payload.error, "Model download failed."));
      }
    });
    return () => {
      void unlistenP.then((fn) => fn());
    };
  }, [refreshDictationModel]);

  useEffect(() => {
    if (resumeSynced.current) return;
    if (!snapshot) return;
    resumeSynced.current = true;
    const next = computeResumeProgress(initialProgress ?? loadOnboardingProgress(), {
      workspaceOnly,
      snapshot,
    });
    setProgress(next);
    saveOnboardingProgress(next);
    setStep(next.currentStep);
    setAuthMode(next.authMode);
  }, [initialProgress, workspaceOnly, snapshot]);

  useEffect(() => {
    saveOnboardingProgress({ authMode });
  }, [authMode]);

  const dictationModelInstalled = dictationModel?.installed ?? false;
  const permsReady = micGranted && (isWindows || (accGranted && imGranted));
  const stepIndex = visStepIndex(step);

  const applyProgress = useCallback((patch: Partial<OnboardingProgress>) => {
    const updated = saveOnboardingProgress(patch);
    setProgress(updated);
    return updated;
  }, []);

  const goToStep = useCallback(
    (target: Step, opts?: { manual?: boolean; authMode?: "personal" | "workspace" }) => {
      if (opts?.manual) setUserNavigatedManually(true);
      else setUserNavigatedManually(false);
      const idx = visStepIndex(target);
      const nextAuth = opts?.authMode ?? authMode;
      if (opts?.authMode) setAuthMode(nextAuth);
      const updated = applyProgress({
        currentStep: target,
        maxStepIndex: workspaceOnly
          ? totalSteps - 1
          : Math.max(progress.maxStepIndex, idx),
        authMode: nextAuth,
      });
      setStep(updated.currentStep);
    },
    [applyProgress, authMode, progress.maxStepIndex, workspaceOnly, totalSteps, visStepIndex],
  );

  const advanceToNextUndone = useCallback(
    (patch?: Partial<Record<OnboardingStep, "done">>) => {
      setUserNavigatedManually(false);
      const status = { ...progress.stepStatus, ...patch };

      if (step === "welcome") status.welcome = "done";
      if (step === "account" && getConnection()) status.account = "done";
      if (step === "permissions" && permsReady) status.permissions = "done";

      let next = firstUndoneStep(status);
      while (next === "account" && !enterpriseRequired && getConnection()) {
        status.account = "done";
        next = firstUndoneStep(status);
      }
      if (!next) {
        applyProgress({
          stepStatus: status,
          maxStepIndex: workspaceOnly ? totalSteps - 1 : progress.maxStepIndex,
        });
        return;
      }

      const nextIdx = visStepIndex(next);
      const updated = applyProgress({
        currentStep: next,
        maxStepIndex: workspaceOnly
          ? totalSteps - 1
          : Math.max(progress.maxStepIndex, nextIdx),
        stepStatus: status,
        authMode,
      });
      setStep(updated.currentStep);
    },
    [
      step,
      progress,
      authMode,
      enterpriseRequired,
      permsReady,
      applyProgress,
      workspaceOnly,
      totalSteps,
      visStepIndex,
    ],
  );

  const completedThroughCurrentStep = useCallback((): Partial<Record<OnboardingStep, "done">> => {
    const done: Partial<Record<OnboardingStep, "done">> = {};
    const currentIdx = visStepIndex(step);
    for (let i = 0; i <= currentIdx; i += 1) {
      const id = visibleStepIds[i];
      if (id) done[id] = "done";
    }
    return done;
  }, [step, visStepIndex, visibleStepIds]);

  const goBack = useCallback(() => {
    const idx = visStepIndex(step);
    if (idx <= 0) return;
    setUserNavigatedManually(true);
    const prev = visibleStepIds[idx - 1];
    const nextAuth = prev === "welcome" ? "personal" : authMode;
    if (nextAuth !== authMode) setAuthMode(nextAuth);
    goToStep(prev, { manual: true, authMode: nextAuth });
  }, [step, authMode, goToStep, visStepIndex, visibleStepIds]);

  const handleStepSelect = useCallback(
    (index: number) => {
      const reachable = Math.min(progress.maxStepIndex, totalSteps - 1);
      const limit = workspaceOnly ? totalSteps - 1 : reachable;
      if (index > limit) return;
      const target = visibleStepIds[index];
      if (!target || target === step) return;
      const nextAuth = target === "welcome" ? "personal" : authMode;
      if (nextAuth !== authMode) setAuthMode(nextAuth);
      goToStep(target, { manual: true, authMode: nextAuth });
    },
    [progress.maxStepIndex, step, authMode, goToStep, workspaceOnly, totalSteps, visibleStepIds],
  );

  useEffect(() => {
    if (!requireLocalModelSetup || workspaceOnly || !isMac || dictationModelInstalled) return;
    if (step === "keys" && progress.stepStatus.keys !== "done") return;
    const updated = applyProgress({
      currentStep: "keys",
      maxStepIndex: Math.max(progress.maxStepIndex, visStepIndex("keys")),
      stepStatus: { ...progress.stepStatus, keys: "pending" },
    });
    setStep(updated.currentStep);
  }, [
    applyProgress,
    isMac,
    progress.maxStepIndex,
    progress.stepStatus,
    requireLocalModelSetup,
    step,
    dictationModelInstalled,
    workspaceOnly,
    visStepIndex,
  ]);

  const maxReachableIndex = workspaceOnly
    ? totalSteps - 1
    : Math.min(progress.maxStepIndex, totalSteps - 1);

  const navProps = {
    steps: ONBOARDING_STEPS.filter((s) => visibleStepIds.includes(s.id)).map((s, index) => ({
      ...s,
      index,
    })),
    currentStepIndex: stepIndex,
    maxReachableIndex,
    stepStatus: shellStepStatus(progress, step),
    onStepSelect: handleStepSelect,
  };

  const handleWorkspaceContinue = useCallback(() => {
    if (!workspacePreview) return;
    onEnterpriseConnected?.(workspacePreview);
    advanceToNextUndone({ account: "done" });
  }, [workspacePreview, onEnterpriseConnected, advanceToNextUndone]);

  const handlePersonalSubmit = useCallback(async () => {
    const trimmedEmail = email.trim();
    if (!trimmedEmail || password.length < 8) {
      setPersonalError("Enter a valid email and an 8+ character password.");
      return;
    }
    setPersonalLoading(true);
    setPersonalError("");
    try {
      const conn = await completeEmailAuth(
        DEFAULT_CLOUD_SERVER_URL,
        trimmedEmail,
        password,
        emailSignup,
      );
      onEnterpriseConnected?.(conn);
      advanceToNextUndone({ account: "done" });
    } catch (e) {
      setPersonalError(
        (e as Error).message ||
          (emailSignup ? "Could not create account." : "Could not sign in."),
      );
    } finally {
      setPersonalLoading(false);
    }
  }, [email, password, emailSignup, onEnterpriseConnected, advanceToNextUndone]);

  useEffect(() => {
    if (userNavigatedManually) return;
    if (step === "permissions" && permsReady) {
      const t = setTimeout(() => {
        advanceToNextUndone({ permissions: "done" });
      }, 700);
      return () => clearTimeout(t);
    }
  }, [
    step,
    permsReady,
    advanceToNextUndone,
    userNavigatedManually,
  ]);

  // Poll the permission snapshot every 3 s while the user is on the permissions
  // step and something is still ungranted. macOS grants made in System Settings
  // otherwise only surface on a window refocus — this catches them without one,
  // so the user never sits on "Waiting for grants…" after actually granting.
  useEffect(() => {
    if (step !== "permissions" || permsReady || !onRefreshPermissions) return;
    const id = setInterval(() => onRefreshPermissions(), 3000);
    return () => clearInterval(id);
  }, [step, permsReady, onRefreshPermissions]);

  // "Try it" live feedback. The polished text is typed straight into the focused
  // textarea (that's the real pipeline confirmation → `dictationTried`), but a
  // failure — mic silence or empty local speech — would otherwise be a
  // silent empty box. Surface phase + errors so the user always knows what
  // happened. Only active on the test step.
  useEffect(() => {
    if (step !== "test") return;
    let noAudioTimer: ReturnType<typeof setTimeout> | null = null;
    const unlistenErr = listen<{ message?: string }>("voice-error", (e) => {
      if (noAudioTimer) clearTimeout(noAudioTimer);
      setTestNoAudio(false);
      setTestPhase("idle");
      setTestError(friendlyError(e.payload?.message, "That didn’t go through. Try again."));
    });
    const unlistenState = listen<{ state?: string }>("app-state", (e) => {
      const s = e.payload?.state;
      if (s === "recording") {
        setTestError("");
        setTestNoAudio(false);
        setTestPhase("recording");
      } else if (s === "processing") {
        setTestPhase("processing");
      } else {
        setTestPhase("idle");
      }
    });
    // When a capture finishes but nothing lands in the box shortly after, nudge
    // the user instead of leaving them staring at an empty field.
    const unlistenDone = listen("voice-done", () => {
      setTestPhase("idle");
      if (noAudioTimer) clearTimeout(noAudioTimer);
      noAudioTimer = setTimeout(() => {
        if (!dictationTriedRef.current) setTestNoAudio(true);
      }, 3500);
    });
    return () => {
      if (noAudioTimer) clearTimeout(noAudioTimer);
      void unlistenErr.then((fn) => fn());
      void unlistenState.then((fn) => fn());
      void unlistenDone.then((fn) => fn());
    };
  }, [step]);

  const chooseLocalEngine = useCallback(async () => {
    if (!dictationModelInstalled) {
      setKeyError("Download the local model first, then continue.");
      return;
    }
    setKeySaving(true);
    setKeyError("");
    try {
      onLocalModelReady?.();
      advanceToNextUndone(completedThroughCurrentStep());
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e);
      setKeyError(message || "Couldn't save your choice. Try again.");
    } finally {
      setKeySaving(false);
    }
  }, [advanceToNextUndone, completedThroughCurrentStep, dictationModelInstalled, onLocalModelReady]);

  const handleDictationDownload = useCallback(async () => {
    setDictationBusy(true);
    setDictationError("");
    setKeyError("");
    try {
      await invoke("download_dictation_model");
      const status = await refreshDictationModel();
      if (status?.installed) onLocalModelReady?.();
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      if (msg !== "cancelled") setDictationError(friendlyError(msg));
    } finally {
      setDictationBusy(false);
    }
  }, [onLocalModelReady, refreshDictationModel]);

  const handleDictationCancel = useCallback(async () => {
    await invoke("meeting_cancel_model_download", { name: NEW_MODEL_FILE }).catch(() => {});
    setDictationDownload(null);
  }, []);

  // Reclaim extra speech-model disk. Oriserve and Silero VAD are preserved.
  const handleReclaimOldModels = useCallback(async () => {
    setReclaiming(true);
    setReclaimError("");
    try {
      const result = await invoke<ReclaimResult>("reclaim_old_models");
      setReclaimResult(result);
    } catch (e) {
      setReclaimError(friendlyError(e));
    } finally {
      setReclaiming(false);
    }
  }, []);

  // Persist the picked hotkey immediately (applied live by the backend on
  // patch). No step advance — the picker sits on the hotkey step and Continue
  // advances. Reflected in local state so the "Try it" step shows the right key.
  const saveHotkeyPref = useCallback(async (id: string) => {
    const updated = await patchPreferences({ record_hotkey: id }, { throwOnError: true });
    if (updated) setPrefs(updated);
  }, []);

  // Final step: the live dictation try-it. Completing it ends onboarding.
  const handleTestComplete = useCallback(() => {
    clearOnboardingProgress();
    onFinish();
  }, [onFinish]);

  // ── Step 1: Welcome ──────────────────────────────────────────────────────
  if (step === "welcome") {
    return (
      <OnboardingShell
        step={stepIndex}
        totalSteps={totalSteps}
        eyebrow="Get started"
        title="Welcome to AirNote."
        subtitle={
          isWindows
            ? "A two-minute setup. Create your account, grant microphone access, install the local speech model, pick a dictation key — then you’ll never type by hand again."
            : "A two-minute setup. Create your account, grant three permissions, install the local speech model, pick a dictation key — then you’ll never type by hand again."
        }
        brandTagline={
          isWindows
            ? "Voice polish for Windows. Hold a key, speak, release — AirNote types polished text into any app."
            : "Voice polish for Mac. Hold a key, speak, release — AirNote types polished text into any app."
        }
        brandKicker={isWindows ? "Built for Windows" : "Built for macOS"}
        brandQuote={
          isWindows
            ? "It’s like typing, except your brain is the keyboard."
            : "Local speech recognition runs on this device."
        }
        bottomNote={<span>{isWindows ? "Windows 10/11" : "macOS 14+"}</span>}
        {...navProps}
      >
        <div className="mt-7 flex flex-col gap-2.5">
          <button onClick={() => advanceToNextUndone({ welcome: "done" })} className="btn-primary btn-lg w-full">
            Get started
            <ArrowRight size={14} />
          </button>
        </div>
      </OnboardingShell>
    );
  }

  // ── Step 2a: Account — personal sign up / log in (default, first-class) ────
  if (step === "account" && authMode === "personal") {
    return (
      <OnboardingShell
        step={stepIndex}
        totalSteps={totalSteps}
        eyebrow="Account"
        title={emailSignup ? "Create your account." : "Welcome back."}
        subtitle={
          emailSignup
            ? "Sign up with email — it takes seconds. No server or setup needed."
            : "Log in to pick up right where you left off."
        }
        brandTagline="Voice polish that just works. Sign in and start dictating in seconds."
        brandKicker="Your account"
        brandQuote="One sign-in, then just hold the key and talk."
        topRight={<span>{stepLabel(step)}</span>}
        bottomNote={<span>Free to start · no credit card</span>}
        onBack={stepIndex > 0 ? goBack : undefined}
        {...navProps}
      >
        <div className="mt-7 flex flex-col gap-3">
          <div className="flex items-center justify-end">
            <button
              type="button"
              className="text-[11px] text-accent hover:underline font-semibold"
              onClick={() => {
                setEmailSignup((v) => !v);
                setPersonalError("");
              }}
            >
              {emailSignup ? "Already have an account? Log in" : "New here? Create account"}
            </button>
          </div>

          <input
            type="email"
            placeholder="you@company.com"
            value={email}
            disabled={personalLoading}
            autoComplete="email"
            onChange={(e) => {
              setEmail(e.target.value);
              setPersonalError("");
            }}
            className="input w-full text-[13px]"
          />
          <input
            type="password"
            placeholder="Password (8+ characters)"
            value={password}
            disabled={personalLoading}
            autoComplete={emailSignup ? "new-password" : "current-password"}
            onChange={(e) => {
              setPassword(e.target.value);
              setPersonalError("");
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") void handlePersonalSubmit();
            }}
            className="input w-full text-[13px]"
          />

          {personalError && (
            <p className="text-[12px] text-center" style={{ color: "hsl(var(--destructive))" }}>
              {personalError}
            </p>
          )}

          <button
            onClick={() => void handlePersonalSubmit()}
            disabled={personalLoading || !email.trim() || password.length < 8}
            className="btn-primary btn-lg w-full"
          >
            {personalLoading ? (
              <Loader2 size={14} className="animate-spin" />
            ) : (
              <ArrowRight size={14} />
            )}
            {personalLoading
              ? emailSignup
                ? "Creating account…"
                : "Signing in…"
              : emailSignup
                ? "Create account"
                : "Sign in"}
          </button>

          <div className="flex items-center gap-3 my-1">
            <div className="h-px flex-1" style={{ background: "hsl(var(--border))" }} />
            <span
              className="text-[10px] uppercase tracking-[0.12em]"
              style={{ color: "hsl(var(--muted-foreground))" }}
            >
              or
            </span>
            <div className="h-px flex-1" style={{ background: "hsl(var(--border))" }} />
          </div>

          <button
            type="button"
            onClick={() => {
              setAuthMode("workspace");
              setPersonalError("");
            }}
            className="w-full rounded-lg border px-3 py-2.5 transition-colors text-center"
            style={{ borderColor: "hsl(var(--border))", color: "hsl(var(--foreground))" }}
          >
            <span className="block text-[12px] font-medium" style={{ color: "hsl(var(--muted-foreground))" }}>
              Setting up for your organization?
            </span>
            <span className="block text-[12px] font-semibold mt-0.5" style={{ color: "hsl(var(--primary))" }}>
              Connect a workspace →
            </span>
          </button>
        </div>
      </OnboardingShell>
    );
  }

  // ── Step 2b: Account — connect workspace (secondary, org server) ───────────
  if (step === "account") {
    const workspaceBack = () => {
      setAuthMode("personal");
      setWorkspacePreview(null);
    };
    return (
      <OnboardingShell
        step={stepIndex}
        totalSteps={totalSteps}
        eyebrow="Workspace"
        title={workspacePreview ? "You're connected." : "Connect your workspace."}
        subtitle={
          workspacePreview
            ? "Signed in to your organization. Continue setup on the next step."
            : "Sign in with your Lark account to connect your workspace."
        }
        brandTagline="Enterprise AirNote runs on your organization's server — your data stays in your workspace."
        brandKicker="Workspace sign-in"
        brandQuote="One workspace login, then every device knows who you are."
        topRight={<span>{stepLabel(step)}</span>}
        bottomNote={
          workspaceOnly ? (
            <span>Reconnect to continue using AirNote</span>
          ) : (
            <span>Managed by your organization</span>
          )
        }
        onBack={workspaceBack}
        {...navProps}
      >
        {workspacePreview ? (
          <div className="mt-7 flex flex-col gap-4">
            <div className="panel overflow-hidden">
              <div className="flex items-center gap-4 px-5 py-4">
                <div
                  className="w-9 h-9 rounded-xl flex items-center justify-center flex-shrink-0 overflow-hidden"
                  style={{
                    background: "hsl(var(--surface-4))",
                    color: "hsl(var(--accent-violet))",
                  }}
                >
                  {workspacePreview.larkAvatarUrl ? (
                    <img src={workspacePreview.larkAvatarUrl} alt="" className="w-full h-full object-cover" />
                  ) : (
                    <Link size={16} />
                  )}
                </div>
                <div className="flex-1 min-w-0">
                  <p className="text-[13px] font-medium text-foreground truncate">
                    {workspacePreview.orgName ?? "Enterprise"}
                  </p>
                  <p className="text-[12px] text-muted-foreground mt-0.5 truncate">
                    {workspacePreview.larkName
                      ? `${workspacePreview.larkName} · ${workspacePreview.email}`
                      : workspacePreview.email}
                  </p>
                </div>
              </div>
              <div className="mx-5 border-t" style={{ borderColor: "hsl(var(--surface-3))" }} />
              <div className="flex items-center gap-3 px-5 py-3.5">
                <Wifi size={14} className="text-muted-foreground shrink-0" />
                <p className="text-[11px] text-muted-foreground truncate">{workspacePreview.serverUrl}</p>
              </div>
            </div>

            <button onClick={handleWorkspaceContinue} className="btn-primary btn-lg w-full">
              {workspaceOnly ? "Continue to AirNote" : "Continue setup"}
              <ArrowRight size={14} />
            </button>

            <button
              onClick={() => setWorkspacePreview(null)}
              className="text-[11px] text-muted-foreground hover:text-foreground text-center transition-colors flex items-center justify-center gap-1"
            >
              <LogOut size={11} />
              Use a different account
            </button>
          </div>
        ) : (
          <div className="mt-7 flex flex-col gap-3">
            <EnterpriseConnectForm
              compact
              variant="onboarding"
              lockedServerUrl={DEFAULT_CLOUD_SERVER_URL}
              allowCustomServerUrl
              onConnected={setWorkspacePreview}
              onCancel={workspaceBack}
            />
            {!enterpriseRequired && (
              <button
                type="button"
                className="w-full rounded-lg border px-3 py-2.5 transition-colors text-center"
                style={{ borderColor: "hsl(var(--border))", color: "hsl(var(--foreground))" }}
                onClick={workspaceBack}
              >
                <span className="block text-[12px] font-medium" style={{ color: "hsl(var(--muted-foreground))" }}>
                  Not using Lark?
                </span>
                <span className="block text-[12px] font-semibold mt-0.5" style={{ color: "hsl(var(--primary))" }}>
                  Continue with email →
                </span>
              </button>
            )}
          </div>
        )}
      </OnboardingShell>
    );
  }

  // ── Step 3: Permissions ──────────────────────────────────────────────────
  if (step === "permissions") {
    // Windows needs only microphone; Accessibility / Input Monitoring are
    // macOS-only TCC gates (SendInput + WH_KEYBOARD_LL need no grant).
    const allGranted = micGranted && (isWindows || (accGranted && imGranted));
    return (
      <OnboardingShell
        step={stepIndex}
        totalSteps={totalSteps}
        eyebrow="Permissions"
        title={isWindows ? "One system grant." : "A few system grants."}
        subtitle={
          isWindows
            ? "Just microphone access. AirNote detects the grant automatically."
            : "Three macOS permissions — one click each. AirNote detects each grant automatically."
        }
        brandTagline={
          isWindows
            ? "Windows asks for microphone access the first time you record. AirNote detects the grant the moment it happens."
            : "macOS will ask you once for each permission. AirNote detects each grant the moment it happens."
        }
        brandKicker="Privacy"
        brandQuote={
          isWindows
            ? "Audio goes only to your selected speech provider. Nothing is stored on our servers."
            : "Speech recognition runs locally after the on-device model is installed. Nothing is stored on our servers."
        }
        topRight={<span>{stepLabel(step)}</span>}
        bottomNote={<span>Change any time in Settings → Permissions</span>}
        onBack={goBack}
        {...navProps}
      >
        <div className="mt-7">
          <PermRow
            icon={<Mic size={15} />}
            title="Microphone"
            desc="Capture audio while you hold the hotkey."
            granted={micGranted}
            onAllow={onMicrophone}
            onOpenSettings={() => void invoke("open_microphone_settings")}
          />
          {!isWindows && (
            <PermRow
              icon={<Shield size={15} />}
              title="Accessibility"
              desc="Type polished text into the focused app."
              granted={accGranted}
              onAllow={onAccessibility}
              onOpenSettings={() => void invoke("open_accessibility_settings")}
            />
          )}
          {!isWindows && (
            <PermRow
              icon={<Keyboard size={15} />}
              title="Input Monitoring"
              desc="Hear your hotkey from any app — even when AirNote isn’t focused."
              granted={imGranted}
              onAllow={onInputMonitoring}
              onOpenSettings={() => void invoke("open_input_monitoring_settings")}
            />
          )}
          {!isWindows && <BrowserContextOnboardingRow />}
        </div>

        <div className="mt-6">
          <button
            onClick={() => advanceToNextUndone({ permissions: "done" })}
            disabled={!allGranted}
            className="btn-primary btn-lg w-full"
          >
            {allGranted ? "Continue" : "Waiting for grants…"}
            {allGranted && <ArrowRight size={14} />}
          </button>
          {!allGranted && (
            <p className="mt-2 text-[11px] text-center" style={{ color: "hsl(var(--muted-foreground))" }}>
              Granted already but still waiting? Use “Open Settings” above, toggle it on, and
              switch back — AirNote re-checks automatically.
            </p>
          )}
        </div>
      </OnboardingShell>
    );
  }

  // ── Step 4: Speech recognition engine ────────────────────────────────────
  // On-device whisper.cpp is required before dictation can run.
  if (step === "keys") {
    const dictationDownloadPct =
      dictationDownload && dictationDownload.total > 0
        ? Math.min(100, Math.round((dictationDownload.received / dictationDownload.total) * 100))
        : null;
    const deviceName = isWindows ? "PC" : "Mac";
    return (
      <OnboardingShell
        step={stepIndex}
        totalSteps={totalSteps}
        eyebrow="Local model"
        title={dictationModelInstalled ? "Local speech model is ready." : "Install the local speech model."}
        subtitle={
          dictationModelInstalled
            ? `AirNote found ${NEW_MODEL_NAME} on this ${deviceName}. Continue when you're ready.`
            : `AirNote transcribes on this ${deviceName}. Download the model before dictation can run.`
        }
        brandTagline={`On-device keeps your voice on this ${deviceName}.`}
        brandKicker="Recommended · on-device"
        brandQuote={`The local model transcribes Hinglish right on your ${deviceName} — private, works offline, no per-use cost.`}
        topRight={<span>{stepLabel(step)}</span>}
        onBack={goBack}
        {...navProps}
      >
        <div className="mt-7 flex flex-col gap-3">
          <div
            className="rounded-xl p-4"
            style={{
              border: "1px solid hsl(var(--primary) / 0.45)",
              background: "hsl(var(--primary) / 0.06)",
            }}
          >
            <div className="flex items-center justify-between mb-1.5 gap-2">
              <div className="flex items-center gap-2 min-w-0">
                <span
                  className="w-[22px] h-[22px] rounded-[7px] grid place-items-center shrink-0"
                  style={{ background: "hsl(var(--primary))", color: "white" }}
                >
                  <Sparkles size={13} />
                </span>
                <p className="text-[13.5px] font-semibold text-foreground truncate">
                Install {NEW_MODEL_NAME}
                </p>
              </div>
              <span
                className="text-[10px] px-1.5 py-0.5 rounded-full font-semibold shrink-0"
                style={{ background: "hsl(var(--primary) / 0.18)", color: "hsl(var(--primary))" }}
              >
                Recommended
              </span>
            </div>
            <p className="text-[11.5px] text-muted-foreground leading-relaxed mb-3">
              Hinglish speech recognition, running entirely on this {deviceName}. Strong on
              Hindi-English code-switching — and your voice never leaves the device. Private,
              offline, no per-use cost.
            </p>
            <DictationModelCard
              modelName={NEW_MODEL_NAME}
              installed={dictationModelInstalled}
              sizeBytes={dictationModel?.size_bytes ?? 0}
              sizeHint={NEW_MODEL_SIZE_HINT}
              progressPct={dictationDownloadPct ?? null}
              busy={dictationBusy}
              error={dictationError}
              onDownload={() => void handleDictationDownload()}
              onCancel={() => void handleDictationCancel()}
            />

            {dictationModelInstalled && (
              <ReclaimOldModelsRow
                reclaiming={reclaiming}
                result={reclaimResult}
                error={reclaimError}
                onReclaim={() => void handleReclaimOldModels()}
              />
            )}

            <button
              onClick={() => void chooseLocalEngine()}
              disabled={keySaving || !dictationModelInstalled}
              className="btn-primary btn-lg w-full mt-3"
            >
              {keySaving
                ? "Saving…"
                : dictationModelInstalled
                  ? "Continue"
                  : `Download ${NEW_MODEL_NAME} · ${NEW_MODEL_SIZE_HINT}`}
              {!keySaving && dictationModelInstalled && <ArrowRight size={14} />}
            </button>
          </div>

          {keyError && (
            <p className="text-[12px] text-center" style={{ color: "hsl(var(--destructive))" }}>
              {keyError}
            </p>
          )}
        </div>
      </OnboardingShell>
    );
  }

  // ── Step 6: Try it — live dictation ──────────────────────────────────────
  // Real dictation: the global hotkey pipeline is already active, so holding
  // the key and speaking types polished text straight into the focused
  // textarea below. This works because the user is now authenticated (account
  // step, required), mic is granted, and (macOS) Accessibility is granted.
  if (step === "test") {
    const hk = prefs?.record_hotkey ?? "caps_lock";
    const hkLabel = hotkeyDisplay(hk, hkPlatform).label;
    const isToggle = hotkeyMode(hk, hkPlatform) === "toggle";
    const trySubtitle = isToggle
      ? `Tap ${hkLabel} to start, speak, then tap again — AirNote types polished text right into the box below. This is the real thing, not a demo.`
      : `Hold ${hkLabel} and speak, then release — AirNote types polished text right into the box below. This is the real thing, not a demo.`;
    const tryPlaceholder = isToggle
      ? `Tap ${hkLabel} and speak — your polished words appear here…`
      : `Hold ${hkLabel} and speak — your polished words appear here…`;
    return (
      <OnboardingShell
        step={stepIndex}
        totalSteps={totalSteps}
        eyebrow="Try it"
        title="Your first dictation."
        subtitle={trySubtitle}
        brandTagline={isToggle ? "Tap to start, speak, tap to send — and watch it land." : "Hold the key, speak, release — and watch it land."}
        brandKicker="Live"
        brandQuote="This is exactly how dictation feels everywhere else in your apps."
        topRight={<span>{stepLabel(step)}</span>}
        onBack={goBack}
        {...navProps}
      >
        <div className="mt-7">
          <div className="onb-try-readaloud">
            <span className="onb-try-readaloud-label">Not sure what to say? Read this aloud</span>
            <span className="onb-try-readaloud-text">“Kal ka demo ready hai — let’s ship it.”</span>
          </div>
          <textarea
            autoFocus
            className="onb-try-field"
            placeholder={tryPlaceholder}
            onChange={(e) => {
              if (e.target.value.trim()) {
                setDictationTried(true);
                dictationTriedRef.current = true;
                setTestError("");
                setTestNoAudio(false);
              }
            }}
          />
          <div className="onb-try-hint">
            <Mic size={13} />
            {testPhase === "recording" ? (
              <>Listening… keep speaking.</>
            ) : testPhase === "processing" ? (
              <>Polishing your words…</>
            ) : isToggle ? (
              <>Tap <span className="onb-try-kbd">{hkLabel}</span> to start, tap again to send.</>
            ) : (
              <>Hold <span className="onb-try-kbd">{hkLabel}</span> and speak, then release.</>
            )}
          </div>
          <ErrorNotice error={testError} />
          {testNoAudio && !testError && (
            <p className="onb-try-warn">
              Didn’t catch anything — click into the box above, then hold the key and speak a
              little louder.
            </p>
          )}
        </div>

        <div className="mt-6 flex flex-col gap-2">
          <button
            onClick={handleTestComplete}
            disabled={!dictationTried}
            className="btn-primary btn-lg w-full"
          >
            {dictationTried ? "Perfect — finish setup" : "Waiting for your first words…"}
            {dictationTried && <ArrowRight size={14} />}
          </button>
          {/* Escape hatch — strongly gated, never a trap (VoiceTypr "Skip for now"). */}
          <button type="button" onClick={handleTestComplete} className="onb-skip-link">
            Skip for now
          </button>
        </div>
      </OnboardingShell>
    );
  }

  // ── Step 5: Hotkey ───────────────────────────────────────────────────────
  const currentHotkey = prefs?.record_hotkey ?? "caps_lock";
  const selected = hotkeyDisplay(currentHotkey, hkPlatform);
  const selectedMode = hotkeyMode(currentHotkey, hkPlatform);

  return (
    <OnboardingShell
      step={stepIndex}
      totalSteps={totalSteps}
      eyebrow="Hotkey"
      title="Pick your dictation key."
      subtitle="Press the key you want to hold — any modifier, Caps Lock, or Fn. You can change it any time."
      brandTagline="One key to dictate. Your thumb learns it in a day."
      brandKicker="Pro tip"
      brandQuote="Most users settle on Caps Lock — it’s right under your finger."
      topRight={<span>{stepLabel(step)}</span>}
      bottomNote={
        <span>
          {selectedMode === "toggle" ? "Tap" : "Hold"} {selected.label} anywhere to dictate, once
          setup is done
        </span>
      }
      onBack={goBack}
      {...navProps}
    >
      <div className="mt-7">
        <HotkeyPicker
          value={currentHotkey}
          onChange={(id) => void saveHotkeyPref(id)}
          platform={hkPlatform}
        />
      </div>

      <div className="mt-6">
        <button
          onClick={() => advanceToNextUndone({ hotkey: "done" })}
          className="btn-primary btn-lg w-full"
        >
          Continue
          <ArrowRight size={14} />
        </button>
      </div>
    </OnboardingShell>
  );
}

// ── Browser context (optional opt-in) ──────────────────────────────────────
// Reuses the PermRow look. "granted" is driven by the opt-in pref rather than an
// OS check (macOS has no Automation preflight); enabling triggers the per-browser
// Automation prompt so it's asked upfront, not mid-dictation.
function BrowserContextOnboardingRow() {
  const [enabled, setEnabled] = useState(false);
  useEffect(() => {
    void getDesktopPrefs().then((p) => setEnabled(p.browser_context_enabled)).catch(() => {});
  }, []);
  const enable = async () => {
    try {
      const p = await getDesktopPrefs();
      await setDesktopPrefs({ ...p, browser_context_enabled: true });
      setEnabled(true);
      void requestBrowserAutomation();
    } catch { /* best-effort */ }
  };
  return (
    <PermRow
      icon={<Link size={15} />}
      title="Browser context (optional)"
      desc="Remember which website you dictate into — the domain only (e.g. mail.google.com), stored on this Mac. You can change this anytime in Settings."
      granted={enabled}
      onAllow={() => void enable()}
      onOpenSettings={() => void enable()}
    />
  );
}

// ── Permission row ──────────────────────────────────────────────────────────

function PermRow({
  icon, title, desc, granted, onAllow, onOpenSettings,
}: {
  icon: React.ReactNode;
  title: string;
  desc: string;
  granted: boolean;
  onAllow: () => void;
  /** Recovery: open the OS privacy pane so the user can flip the grant by hand. */
  onOpenSettings?: () => void;
}) {
  const [attempted, setAttempted] = useState(false);
  return (
    <div className="onb-perm-wrap">
      <div className="onb-row">
        <span className="ico-wrap">{icon}</span>
        <div>
          <div className="row-title">{title}</div>
          <div className="row-desc">{desc}</div>
        </div>
        {granted ? (
          <span className="accent-pill" style={{ color: "hsl(140 65% 65%)", background: "hsl(140 65% 50% / 0.14)" }}>
            Granted
          </span>
        ) : (
          <button
            onClick={() => {
              setAttempted(true);
              onAllow();
            }}
            className="btn-ghost text-[11.5px]"
            style={{ height: 28 }}
          >
            Allow
          </button>
        )}
      </div>
      {!granted && onOpenSettings && (
        <button type="button" onClick={onOpenSettings} className="onb-perm-settings-link">
          <ExternalLink size={11} />
          {attempted ? "Didn’t work? Open System Settings" : "Open System Settings"}
        </button>
      )}
    </div>
  );
}

// ── Local model setup card ───────────────────────────────────────────────────

function DictationModelCard({
  modelName,
  installed,
  sizeBytes,
  sizeHint,
  progressPct,
  busy,
  error,
  onDownload,
  onCancel,
}: {
  modelName: string;
  installed: boolean;
  sizeBytes: number;
  sizeHint: string;
  progressPct: number | null;
  busy: boolean;
  error: string;
  onDownload: () => void;
  onCancel: () => void;
}) {
  const downloading = progressPct !== null && !installed;
  return (
    <div
      className="rounded-lg p-3"
      style={{
        background: "hsl(0 0% 100% / 0.025)",
        boxShadow: "inset 0 0 0 1px hsl(var(--glass-stroke))",
      }}
    >
      <div className="flex items-center justify-between gap-3">
        <div className="flex items-center gap-2.5 min-w-0">
          <span
            className="w-[20px] h-[20px] rounded-[6px] grid place-items-center shrink-0"
            style={{ background: "#6c5ce7", color: "white" }}
          >
            <Cpu size={12} />
          </span>
          <span className="text-[12.5px] font-medium truncate" style={{ color: "hsl(var(--foreground))" }}>
            {installed
              ? `${modelName} · ${sizeBytes > 0 ? formatSize(sizeBytes) : sizeHint}`
              : downloading
                ? `Downloading ${modelName}…`
                : `${modelName} · ${sizeHint}`}
          </span>
        </div>

        {installed ? (
          <span
            className="accent-pill shrink-0"
            style={{ color: "hsl(140 65% 65%)", background: "hsl(140 65% 50% / 0.14)" }}
          >
            <Check size={11} /> Installed
          </span>
        ) : downloading ? (
          <button
            type="button"
            onClick={onCancel}
            className="btn-ghost text-[11px] shrink-0"
            style={{ height: 28 }}
          >
            <X size={12} />
            Cancel
          </button>
        ) : (
          <button
            type="button"
            onClick={onDownload}
            disabled={busy}
            className="btn-ghost text-[11px] shrink-0"
            style={{ height: 28 }}
          >
            {busy ? <Loader2 size={12} className="animate-spin" /> : <Download size={12} />}
            Download
          </button>
        )}
      </div>

      {downloading && (
        <div className="mt-3">
          <div className="h-1.5 rounded-full overflow-hidden" style={{ background: "hsl(var(--surface-3))" }}>
            <div
              className="h-full rounded-full transition-all"
              style={{
                width: `${Math.max(4, progressPct ?? 0)}%`,
                background: "hsl(var(--primary))",
              }}
            />
          </div>
          <p className="text-[11px] mt-1" style={{ color: "hsl(var(--muted-foreground))" }}>
            Downloading {progressPct ?? 0}%
          </p>
        </div>
      )}

      <ErrorNotice error={error} onRetry={onDownload} className="mt-2" />
    </div>
  );
}

// ── API key card ────────────────────────────────────────────────────────────
