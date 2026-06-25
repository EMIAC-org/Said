import type { AppSnapshot, Preferences } from "@/types";
import { getConnection, loadSavedAuthMode } from "@/lib/enterprise";

export type OnboardingStep = "welcome" | "account" | "permissions" | "keys" | "hotkey";
export type OnboardingAuthMode = "personal" | "workspace";
export type StepCompletion = "pending" | "done";

const STORAGE_KEY = "said:onboarding-progress";

export const ONBOARDING_STEPS: { id: OnboardingStep; label: string }[] = [
  { id: "welcome", label: "Welcome" },
  { id: "account", label: "Account" },
  { id: "permissions", label: "Permissions" },
  { id: "keys", label: "Keys" },
  { id: "hotkey", label: "Hotkey" },
];

export const ONBOARDING_STEP_IDS = ONBOARDING_STEPS.map((s) => s.id);

function emptyStepStatus(): Record<OnboardingStep, StepCompletion> {
  return {
    welcome: "pending",
    account: "pending",
    permissions: "pending",
    keys: "pending",
    hotkey: "pending",
  };
}

export interface OnboardingProgress {
  version: 1;
  currentStep: OnboardingStep;
  maxStepIndex: number;
  authMode: OnboardingAuthMode;
  stepStatus: Record<OnboardingStep, StepCompletion>;
  updatedAt: number;
}

export function defaultOnboardingProgress(
  overrides?: Partial<Pick<OnboardingProgress, "currentStep" | "authMode" | "maxStepIndex">>,
): OnboardingProgress {
  const currentStep = overrides?.currentStep ?? "welcome";
  const idx = ONBOARDING_STEP_IDS.indexOf(currentStep);
  return {
    version: 1,
    currentStep,
    maxStepIndex: overrides?.maxStepIndex ?? Math.max(0, idx),
    authMode: overrides?.authMode ?? "personal",
    stepStatus: emptyStepStatus(),
    updatedAt: Date.now(),
  };
}

function isStep(value: unknown): value is OnboardingStep {
  return typeof value === "string" && ONBOARDING_STEP_IDS.includes(value as OnboardingStep);
}

function normalizeProgress(raw: Partial<OnboardingProgress>): OnboardingProgress {
  const base = defaultOnboardingProgress();
  const currentStep = isStep(raw.currentStep) ? raw.currentStep : base.currentStep;
  const stepIdx = ONBOARDING_STEP_IDS.indexOf(currentStep);
  const maxStepIndex =
    typeof raw.maxStepIndex === "number" && Number.isFinite(raw.maxStepIndex)
      ? Math.min(Math.max(0, Math.floor(raw.maxStepIndex)), ONBOARDING_STEPS.length - 1)
      : Math.max(0, stepIdx);
  const authMode = raw.authMode === "workspace" ? "workspace" : "personal";
  const stepStatus = { ...emptyStepStatus() };
  if (raw.stepStatus && typeof raw.stepStatus === "object") {
    for (const id of ONBOARDING_STEP_IDS) {
      const v = (raw.stepStatus as Record<string, StepCompletion>)[id];
      if (v === "done") stepStatus[id] = "done";
    }
  }
  return {
    version: 1,
    currentStep,
    maxStepIndex: Math.max(maxStepIndex, stepIdx),
    authMode,
    stepStatus,
    updatedAt: typeof raw.updatedAt === "number" ? raw.updatedAt : Date.now(),
  };
}

export function loadOnboardingProgress(): OnboardingProgress | null {
  try {
    const text = localStorage.getItem(STORAGE_KEY);
    if (!text) return null;
    const parsed = JSON.parse(text) as Partial<OnboardingProgress>;
    return normalizeProgress(parsed);
  } catch {
    return null;
  }
}

export function saveOnboardingProgress(patch: Partial<OnboardingProgress>): OnboardingProgress {
  const prev = loadOnboardingProgress() ?? defaultOnboardingProgress();
  const merged = normalizeProgress({ ...prev, ...patch, updatedAt: Date.now() });
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(merged));
  } catch {
    // ignore quota errors
  }
  return merged;
}

export function clearOnboardingProgress(): void {
  try {
    localStorage.removeItem(STORAGE_KEY);
  } catch {
    // ignore
  }
}

export function stepIndex(step: OnboardingStep): number {
  return ONBOARDING_STEP_IDS.indexOf(step);
}

/** First step in wizard order that is not marked done. */
export function firstUndoneStep(
  status: Record<OnboardingStep, StepCompletion>,
): OnboardingStep | null {
  for (const id of ONBOARDING_STEP_IDS) {
    if (status[id] !== "done") return id;
  }
  return null;
}

export function allStepsDone(status: Record<OnboardingStep, StepCompletion>): boolean {
  return firstUndoneStep(status) === null;
}

export function permissionsReady(snapshot: AppSnapshot | null): boolean {
  if (!snapshot?.microphone_granted) return false;
  if (snapshot.platform === "windows") return true;
  return !!snapshot.accessibility_granted && !!snapshot.input_monitoring_granted;
}

export function keysReady(prefs: Preferences | null): boolean {
  return !!prefs?.groq_api_key?.trim();
}

/** Reconcile stored progress with live prefs/session on resume. */
export function computeResumeProgress(
  stored: OnboardingProgress | null,
  opts: {
    workspaceOnly: boolean;
    snapshot: AppSnapshot | null;
    prefs: Preferences | null;
  },
): OnboardingProgress {
  const savedAuth = loadSavedAuthMode();
  if (opts.workspaceOnly) {
    const maxIdx = ONBOARDING_STEPS.length - 1;
    const status = emptyStepStatus();
    status.welcome = "done";
    status.permissions = permissionsReady(opts.snapshot) ? "done" : "pending";
    status.keys = keysReady(opts.prefs) ? "done" : "pending";
    status.hotkey = "done";
    status.account = getConnection() ? "done" : "pending";

    const storedStep = stored?.currentStep;
    const currentStep =
      storedStep && stepIndex(storedStep) <= maxIdx ? storedStep : "account";

    return normalizeProgress({
      currentStep,
      maxStepIndex: maxIdx,
      authMode: savedAuth ?? stored?.authMode ?? "personal",
      stepStatus: status,
    });
  }

  let progress = stored ?? defaultOnboardingProgress();
  const status = { ...progress.stepStatus };

  if (getConnection()) {
    status.account = "done";
    progress.maxStepIndex = Math.max(progress.maxStepIndex, stepIndex("account"));
  } else if (stepIndex(progress.currentStep) > stepIndex("account")) {
    progress.currentStep = "account";
  }

  if (permissionsReady(opts.snapshot)) {
    status.permissions = "done";
    progress.maxStepIndex = Math.max(progress.maxStepIndex, stepIndex("permissions"));
  }

  if (keysReady(opts.prefs)) {
    status.keys = "done";
    progress.maxStepIndex = Math.max(progress.maxStepIndex, stepIndex("keys"));
  }

  return normalizeProgress({
    ...progress,
    authMode: savedAuth ?? progress.authMode,
    stepStatus: status,
  });
}

export function shellStepStatus(
  progress: OnboardingProgress,
  currentStep: OnboardingStep,
): Record<OnboardingStep, "pending" | "done" | "current"> {
  const out = {} as Record<OnboardingStep, "pending" | "done" | "current">;
  for (const id of ONBOARDING_STEP_IDS) {
    if (id === currentStep) out[id] = "current";
    else if (progress.stepStatus[id] === "done") out[id] = "done";
    else out[id] = "pending";
  }
  return out;
}
