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
import { getPreferences, invoke, patchPreferences } from "@/lib/invoke";
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
  /** When true, user must connect workspace before continuing setup. */
  enterpriseRequired?: boolean;
  /** Called when workspace OAuth completes. */
  onEnterpriseConnected?: (conn: EnterpriseConnection) => void;
  /** Reconnect-only mode — skip welcome/permissions/keys/hotkey. */
  workspaceOnly?: boolean;
  /** Restored onboarding progress from localStorage. */
  initialProgress?: OnboardingProgress | null;
  /** Existing macOS users must install the local Swift model before continuing. */
  requireLocalModelSetup?: boolean;
  /** Called once the local Swift model is installed. */
  onLocalModelReady?: () => void;
}

// The one on-device dictation model (Oriserve Hinglish GGML, ~148 MB). The Silero
// VAD is bundled with the app and auto-fetched after this download, so onboarding
// only ever surfaces this single model.
const DICTATION_MODEL_NAME = "ggml-oriserve-hinglish-fp16.bin";

interface SwiftModelStatus {
  installed: boolean;
  size_bytes: number;
  path: string;
}

// Payload of the shared `meeting-model-download` event (keyed by model name).
interface SwiftDownloadProgress {
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

// The "keys" step is the macOS-only "Speech recognition" step (choose on-device
// Swift vs Cloud). Windows has no on-device dictation option, so that step is
// filtered out of the flow entirely — Windows never sees it and it isn't counted.
const MAC_ONLY_STEPS: ReadonlySet<Step> = new Set<Step>(["keys"]);
function visibleStepsFor(isMac: boolean): Step[] {
  return isMac ? [...STEPS] : STEPS.filter((s) => !MAC_ONLY_STEPS.has(s));
}

// ── Main component ─────────────────────────────────────────────────────────

export function OnboardingFlow({
  snapshot,
  onMicrophone,
  onAccessibility,
  onInputMonitoring,
  onFinish,
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
  const [swiftModel, setSwiftModel] = useState<SwiftModelStatus | null>(null);
  const [swiftDownload, setSwiftDownload] = useState<SwiftDownloadProgress | null>(null);
  const [swiftBusy, setSwiftBusy] = useState(false);
  const [swiftError, setSwiftError] = useState("");
  const [workspacePreview, setWorkspacePreview] = useState<EnterpriseConnection | null>(null);
  const [userNavigatedManually, setUserNavigatedManually] = useState(false);
  const resumeSynced = useRef(false);

  const [progress, setProgress] = useState<OnboardingProgress>(() =>
    computeResumeProgress(initialProgress ?? loadOnboardingProgress(), {
      workspaceOnly,
      snapshot: null,
      prefs: null,
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

  const [step, setStep] = useState<Step>(() => progress.currentStep);

  // Platform-aware step list. On Windows the macOS-only "Speech recognition"
  // (keys) step is filtered out — it is never shown, navigated to, or counted.
  const visibleStepIds = useMemo(() => visibleStepsFor(isMac), [isMac]);
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

  const refreshSwiftModel = useCallback(async () => {
    if (!isMac) {
      setSwiftModel(null);
      return null;
    }
    try {
      const status = await invoke<SwiftModelStatus>("dictation_model_status");
      setSwiftModel(status);
      if (status.installed) onLocalModelReady?.();
      return status;
    } catch (e) {
      setSwiftError(e instanceof Error ? e.message : String(e));
      return null;
    }
  }, [isMac, onLocalModelReady]);

  useEffect(() => {
    void refreshSwiftModel();
  }, [refreshSwiftModel]);

  useEffect(() => {
    if (!isMac) return;
    const unlistenP = listen<SwiftDownloadProgress>("meeting-model-download", (event) => {
      const payload = event.payload;
      // The event is shared across models (dictation + VAD); only react to the
      // dictation model so VAD's silent auto-fetch never shows in onboarding.
      if (payload.name !== DICTATION_MODEL_NAME) return;
      if (payload.status === "downloading") {
        setSwiftDownload(payload);
        setSwiftError("");
      } else {
        setSwiftDownload(null);
      }
      if (payload.status === "done") {
        void refreshSwiftModel();
      }
      if (payload.status === "error") {
        setSwiftError(payload.error || "Model download failed.");
      }
    });
    return () => {
      void unlistenP.then((fn) => fn());
    };
  }, [isMac, refreshSwiftModel]);

  useEffect(() => {
    if (resumeSynced.current) return;
    if (!prefs && !snapshot) return;
    resumeSynced.current = true;
    const next = computeResumeProgress(initialProgress ?? loadOnboardingProgress(), {
      workspaceOnly,
      snapshot,
      prefs,
    });
    setProgress(next);
    saveOnboardingProgress(next);
    setStep(next.currentStep);
    setAuthMode(next.authMode);
  }, [initialProgress, workspaceOnly, snapshot, prefs]);

  useEffect(() => {
    saveOnboardingProgress({ authMode });
  }, [authMode]);

  const swiftInstalled = isMac ? (swiftModel?.installed ?? false) : true;
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
      // Windows has no on-device dictation — the macOS-only "Speech recognition"
      // (keys) step is skipped automatically so it never becomes current.
      while (next === "keys" && !isMac) {
        status.keys = "done";
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
      isMac,
      applyProgress,
      workspaceOnly,
      totalSteps,
      visStepIndex,
    ],
  );

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
    if (!requireLocalModelSetup || workspaceOnly || !isMac || swiftInstalled) return;
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
    swiftInstalled,
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

  // No API keys are collected anymore — they're bundled into the build. This
  // macOS-only step just records which speech engine the user wants.
  const chooseCloudEngine = useCallback(async () => {
    setKeySaving(true);
    setKeyError("");
    try {
      const updated = await patchPreferences({ stt_provider: "deepgram" });
      if (updated) setPrefs(updated);
      advanceToNextUndone({ keys: "done" });
    } catch {
      setKeyError("Couldn't save your choice. Try again.");
    } finally {
      setKeySaving(false);
    }
  }, [advanceToNextUndone]);

  const chooseLocalEngine = useCallback(async () => {
    if (!swiftInstalled) {
      setKeyError("Download the local model first, then continue.");
      return;
    }
    setKeySaving(true);
    setKeyError("");
    try {
      const updated = await patchPreferences({ stt_provider: "whisper_local" });
      if (updated) {
        setPrefs(updated);
        onLocalModelReady?.();
      }
      advanceToNextUndone({ keys: "done" });
    } catch {
      setKeyError("Couldn't save your choice. Try again.");
    } finally {
      setKeySaving(false);
    }
  }, [advanceToNextUndone, onLocalModelReady, swiftInstalled]);

  const handleSwiftDownload = useCallback(async () => {
    setSwiftBusy(true);
    setSwiftError("");
    setKeyError("");
    try {
      await invoke("download_dictation_model");
      const status = await refreshSwiftModel();
      if (status?.installed) onLocalModelReady?.();
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      if (msg !== "cancelled") setSwiftError(msg);
    } finally {
      setSwiftBusy(false);
    }
  }, [onLocalModelReady, refreshSwiftModel]);

  const handleSwiftCancel = useCallback(async () => {
    await invoke("meeting_cancel_model_download", { name: DICTATION_MODEL_NAME }).catch(() => {});
    setSwiftDownload(null);
  }, []);

  const handleHotkeySelect = useCallback(async (key: string) => {
    await patchPreferences({ record_hotkey: key });
    if (workspaceOnly) {
      advanceToNextUndone({ hotkey: "done" });
      return;
    }
    clearOnboardingProgress();
    onFinish();
  }, [workspaceOnly, advanceToNextUndone, onFinish]);

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
            ? "A two-minute setup. Create your account, grant microphone access, pick a hold-key — then you’ll never type by hand again."
            : "A two-minute setup. Create your account, grant three permissions, choose on-device or cloud speech recognition, pick a hold-key — then you’ll never type by hand again."
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
            : "Local speech recognition first. Cloud STT only if you choose it later."
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
            className="w-full rounded-lg border px-3 py-2.5 text-[12px] font-semibold transition-colors flex items-center justify-center gap-2"
            style={{ borderColor: "hsl(var(--border))", color: "hsl(var(--foreground))" }}
          >
            Setting up for your organization?
            <span style={{ color: "hsl(var(--primary))" }}>Connect a workspace →</span>
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
            : "Enter your organization's server URL, then sign in. For teams on a self-hosted AirNote server."
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
              onConnected={setWorkspacePreview}
              onCancel={workspaceBack}
            />
            {workspaceOnly && (
              <button
                type="button"
                className="text-[11px] text-center text-muted-foreground hover:text-foreground transition-colors"
                onClick={workspaceBack}
              >
                Sign in with email instead
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
            : "Speech recognition runs locally after the Swift model is installed. Nothing is stored on our servers."
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
          />
          {!isWindows && (
            <PermRow
              icon={<Shield size={15} />}
              title="Accessibility"
              desc="Type polished text into the focused app."
              granted={accGranted}
              onAllow={onAccessibility}
            />
          )}
          {!isWindows && (
            <PermRow
              icon={<Keyboard size={15} />}
              title="Input Monitoring"
              desc="Hear your hotkey from any app — even when AirNote isn’t focused."
              granted={imGranted}
              onAllow={onInputMonitoring}
            />
          )}
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
        </div>
      </OnboardingShell>
    );
  }

  // ── Step 4 (macOS only): Speech recognition engine ───────────────────────
  // On-device Swift vs Cloud Deepgram. No API keys — they're bundled into the
  // build. Windows never reaches this step (it's filtered out of the flow);
  // Windows dictation always uses Cloud.
  if (step === "keys" && isMac) {
    const swiftDownloadPct =
      swiftDownload && swiftDownload.total > 0
        ? Math.min(100, Math.round((swiftDownload.received / swiftDownload.total) * 100))
        : null;
    return (
      <OnboardingShell
        step={stepIndex}
        totalSteps={totalSteps}
        eyebrow="Speech recognition"
        title="How should AirNote hear you?"
        subtitle="Run speech recognition on this Mac, or in the cloud. You can switch any time in Settings."
        brandTagline="On-device keeps your voice on this Mac. Cloud is instant with nothing to download."
        brandKicker="Recommended · on-device"
        brandQuote="The local model transcribes Hinglish right on your Mac — private, works offline, no per-use cost."
        topRight={<span>{stepLabel(step)}</span>}
        onBack={goBack}
        {...navProps}
      >
        <div className="mt-7 flex flex-col gap-3">
          {/* On-device (recommended) */}
          <div
            className="rounded-xl p-4"
            style={{
              border: "1px solid hsl(var(--primary) / 0.4)",
              background: "hsl(var(--primary) / 0.04)",
            }}
          >
            <div className="flex items-center justify-between mb-1.5">
              <p className="text-[13px] font-semibold text-foreground">On-device</p>
              <span
                className="text-[10px] px-1.5 py-0.5 rounded-full font-medium"
                style={{ background: "hsl(var(--primary) / 0.15)", color: "hsl(var(--primary))" }}
              >
                Recommended
              </span>
            </div>
            <p className="text-[11.5px] text-muted-foreground leading-relaxed mb-3">
              Transcribes Hinglish on your Mac — works offline, nothing leaves the device, and there’s
              no per-use cost. One-time ~148 MB download.
            </p>
            <SwiftModelCard
              installed={swiftInstalled}
              sizeBytes={swiftModel?.size_bytes ?? 0}
              progressPct={swiftDownloadPct ?? null}
              busy={swiftBusy}
              error={swiftError}
              onDownload={() => void handleSwiftDownload()}
              onCancel={() => void handleSwiftCancel()}
            />
            <button
              onClick={() => void chooseLocalEngine()}
              disabled={keySaving || !swiftInstalled}
              className="btn-primary btn-lg w-full mt-3"
            >
              {keySaving
                ? "Saving…"
                : swiftInstalled
                  ? "Use on-device model"
                  : "Download to use on-device"}
              {!keySaving && swiftInstalled && <ArrowRight size={14} />}
            </button>
          </div>

          {/* Cloud */}
          <div
            className="rounded-xl p-4"
            style={{ border: "1px solid hsl(var(--surface-3))", background: "hsl(var(--surface-2))" }}
          >
            <p className="text-[13px] font-semibold text-foreground mb-1.5">Cloud (Deepgram)</p>
            <p className="text-[11.5px] text-muted-foreground leading-relaxed mb-3">
              Instant — nothing to download. Needs an internet connection while you dictate.
            </p>
            <button
              onClick={() => void chooseCloudEngine()}
              disabled={keySaving}
              className="w-full rounded-xl px-4 py-2.5 text-[13px] font-medium border transition-colors disabled:opacity-50"
              style={{
                borderColor: "hsl(var(--surface-3))",
                background: "hsl(var(--surface-3))",
                color: "hsl(var(--foreground))",
              }}
            >
              {keySaving ? "Saving…" : "Use cloud instead"}
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

  // ── Step 5: Hotkey ───────────────────────────────────────────────────────
  const currentHotkey = prefs?.record_hotkey ?? "caps_lock";
  const options: { key: string; glyph: string; label: string; desc: string }[] = [
    { key: "caps_lock",    glyph: "⇪",  label: "Caps Lock",     desc: "Single key, easy to hold." },
    { key: "right_option", glyph: isWindows ? "Alt" : "⌥",  label: isWindows ? "Right Alt" : "Right Option",  desc: "Stays out of the way." },
    ...(!isWindows
      ? [{ key: "fn", glyph: "fn", label: "Fn / Globe", desc: "The world key on MacBooks." }]
      : []),
  ];
  // Reflect the actually-selected hold-key (not a hardcoded "Caps Lock").
  const selectedHotkeyLabel = options.find((o) => o.key === currentHotkey)?.label ?? "Caps Lock";

  return (
    <OnboardingShell
      step={stepIndex}
      totalSteps={totalSteps}
      eyebrow="Hotkey"
      title="Pick a hold-key."
      subtitle="Hold this key to record, release to send. You can change it any time."
      brandTagline="Hold to record, release to send. Your thumb learns it in a day."
      brandKicker="Pro tip"
      brandQuote="Most users settle on Caps Lock — it’s already a hold-key for nothing useful."
      topRight={<span>{stepLabel(step)}</span>}
      bottomNote={<span>Press {selectedHotkeyLabel} anywhere to dictate, once setup is done</span>}
      onBack={goBack}
      {...navProps}
    >
      <div className="mt-7">
        {options.map((opt) => {
          const isSelected = currentHotkey === opt.key;
          return (
            <div
              key={opt.key}
              role="button"
              tabIndex={0}
              onClick={() => handleHotkeySelect(opt.key)}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  handleHotkeySelect(opt.key);
                }
              }}
              className={`onb-row selectable ${isSelected ? "selected" : ""}`}
            >
              <span className="key-glyph">{opt.glyph}</span>
              <div>
                <div className="row-title">{opt.label}</div>
                <div className="row-desc">{opt.desc}</div>
              </div>
              {isSelected ? (
                <Check size={14} style={{ color: "hsl(var(--primary))" }} />
              ) : (
                <ArrowRight size={14} style={{ color: "hsl(var(--muted-foreground) / 0.6)" }} />
              )}
            </div>
          );
        })}
      </div>

      <div className="mt-6">
        <button
          onClick={() => handleHotkeySelect(currentHotkey)}
          className="btn-primary btn-lg w-full"
        >
          Start using AirNote
          <ArrowRight size={14} />
        </button>
      </div>
    </OnboardingShell>
  );
}

// ── Permission row ──────────────────────────────────────────────────────────

function PermRow({
  icon, title, desc, granted, onAllow,
}: {
  icon: React.ReactNode;
  title: string;
  desc: string;
  granted: boolean;
  onAllow: () => void;
}) {
  return (
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
        <button onClick={onAllow} className="btn-ghost text-[11.5px]" style={{ height: 28 }}>
          Allow
        </button>
      )}
    </div>
  );
}

// ── Local model setup card ───────────────────────────────────────────────────

function SwiftModelCard({
  installed,
  sizeBytes,
  progressPct,
  busy,
  error,
  onDownload,
  onCancel,
}: {
  installed: boolean;
  sizeBytes: number;
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
      <div className="flex items-start justify-between gap-3">
        <div className="flex items-start gap-2 min-w-0">
          <span
            className="w-[18px] h-[18px] rounded-[5px] grid place-items-center text-[10px] font-bold shrink-0"
            style={{ background: "#6c5ce7", color: "white" }}
          >
            <Cpu size={11} />
          </span>
          <div className="min-w-0">
            <div className="flex items-center gap-2">
              <span className="text-[12.5px] font-semibold" style={{ color: "hsl(var(--foreground))" }}>
                On-device model
              </span>
              {installed && (
                <span className="accent-pill" style={{ color: "hsl(140 65% 65%)", background: "hsl(140 65% 50% / 0.14)" }}>
                  Installed
                </span>
              )}
            </div>
            <p className="text-[11.5px] mt-1" style={{ color: "hsl(var(--muted-foreground))" }}>
              On-device speech recognition for this Mac. Your voice never leaves the device; polishing happens after.
            </p>
            {installed && sizeBytes > 0 && (
              <p className="text-[11px] mt-1" style={{ color: "hsl(var(--muted-foreground))" }}>
                {formatSize(sizeBytes)} installed
              </p>
            )}
          </div>
        </div>

        {installed ? (
          <Check size={16} style={{ color: "hsl(140 65% 65%)" }} />
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

      {error && (
        <p className="text-[11.5px] mt-2" style={{ color: "hsl(var(--destructive))" }}>
          {error}
        </p>
      )}
    </div>
  );
}

// ── API key card ────────────────────────────────────────────────────────────
