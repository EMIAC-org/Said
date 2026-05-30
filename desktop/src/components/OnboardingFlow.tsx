import React, { useCallback, useEffect, useState } from "react";
import {
  ArrowRight,
  Check,
  ExternalLink,
  Eye,
  EyeOff,
  Mic,
  Shield,
  Keyboard,
  Link,
  Wifi,
  LogOut,
} from "lucide-react";
import { OnboardingShell } from "@/components/OnboardingShell";
import { EnterpriseConnectForm } from "@/components/EnterpriseConnectForm";
import type { EnterpriseConnection } from "@/lib/enterprise";
import { getConnection } from "@/lib/enterprise";
import type { AppSnapshot, Preferences } from "@/types";
import { getPreferences, patchPreferences, openExternal } from "@/lib/invoke";

type Step = "welcome" | "workspace" | "permissions" | "keys" | "hotkey";

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
}

const STEPS: Step[] = ["welcome", "workspace", "permissions", "keys", "hotkey"];
const TOTAL_STEPS = STEPS.length;

function stepLabel(step: Step): string {
  return `${STEPS.indexOf(step) + 1} of ${TOTAL_STEPS}`;
}

// ── Password-style input with show/hide ─────────────────────────────────────

function KeyInput({
  value,
  onChange,
  placeholder,
}: {
  value: string;
  onChange: (v: string) => void;
  placeholder: string;
}) {
  const [show, setShow] = useState(false);
  return (
    <div className="relative">
      <input
        type={show ? "text" : "password"}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        className="input pr-10"
        style={{ fontFamily: "ui-monospace, SF Mono, Menlo, monospace" }}
      />
      <button
        type="button"
        onClick={() => setShow((s) => !s)}
        className="absolute right-2 top-1/2 -translate-y-1/2 p-1 transition-colors"
        style={{ color: "hsl(var(--muted-foreground))" }}
        aria-label={show ? "Hide" : "Show"}
      >
        {show ? <EyeOff size={13} /> : <Eye size={13} />}
      </button>
    </div>
  );
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
}: Props) {
  const [prefs, setPrefs] = useState<Preferences | null>(null);
  const [groqKey, setGroqKey] = useState("");
  const [deepgramKey, setDeepgramKey] = useState("");
  const [keySaving, setKeySaving] = useState(false);
  const [keyError, setKeyError] = useState("");
  const [workspacePreview, setWorkspacePreview] = useState<EnterpriseConnection | null>(null);

  const micGranted = snapshot?.microphone_granted ?? false;
  const accGranted = snapshot?.accessibility_granted ?? false;
  const imGranted = snapshot?.input_monitoring_granted ?? false;
  const isWindows = snapshot?.platform === "windows";

  useEffect(() => {
    getPreferences().then((p) => {
      if (p) {
        setPrefs(p);
        if (p.groq_api_key) setGroqKey(p.groq_api_key);
        if (p.deepgram_api_key) setDeepgramKey(p.deepgram_api_key);
      }
    });
  }, []);

  const hasKeys = !!(prefs?.groq_api_key && prefs?.deepgram_api_key);

  const [step, setStep] = useState<Step>(() => (workspaceOnly ? "workspace" : "welcome"));

  const stepIndex = STEPS.indexOf(step);

  const goNext = useCallback(() => {
    const idx = STEPS.indexOf(step);
    if (idx >= STEPS.length - 1) return;
    let next = STEPS[idx + 1];
    if (next === "workspace" && !enterpriseRequired && getConnection()) {
      next = STEPS[idx + 2] ?? next;
    }
    setStep(next);
  }, [step, enterpriseRequired]);

  const goBack = useCallback(() => {
    const idx = STEPS.indexOf(step);
    if (idx > 0) setStep(STEPS[idx - 1]);
  }, [step]);

  const handleWorkspaceContinue = useCallback(() => {
    if (!workspacePreview) return;
    onEnterpriseConnected?.(workspacePreview);
    if (workspaceOnly) return;
    goNext();
  }, [workspacePreview, onEnterpriseConnected, workspaceOnly, goNext]);

  // Auto-advance the permissions step once the required grants are in (delay so
  // the user sees the green check before the screen swaps). Windows only needs
  // microphone — Accessibility / Input Monitoring are macOS-only TCC gates.
  useEffect(() => {
    const permsReady = micGranted && (isWindows || (accGranted && imGranted));
    if (step === "permissions" && permsReady) {
      const t = setTimeout(goNext, 700);
      return () => clearTimeout(t);
    }
    if (step === "keys" && hasKeys) {
      const t = setTimeout(goNext, 300);
      return () => clearTimeout(t);
    }
  }, [step, micGranted, accGranted, imGranted, isWindows, hasKeys, goNext]);

  const handleSaveKeys = useCallback(async () => {
    if (!groqKey.trim() || !deepgramKey.trim()) {
      setKeyError("Both keys are required.");
      return;
    }
    setKeySaving(true);
    setKeyError("");
    try {
      const updated = await patchPreferences({
        groq_api_key: groqKey.trim(),
        deepgram_api_key: deepgramKey.trim(),
        llm_provider: "groq",
      });
      if (updated) setPrefs(updated);
      goNext();
    } catch {
      setKeyError("Failed to save keys. Try again.");
    } finally {
      setKeySaving(false);
    }
  }, [groqKey, deepgramKey, goNext]);

  const handleHotkeySelect = useCallback(async (key: string) => {
    await patchPreferences({ record_hotkey: key });
    onFinish();
  }, [onFinish]);

  // ── Step 1: Welcome ──────────────────────────────────────────────────────
  if (step === "welcome") {
    return (
      <OnboardingShell
        step={stepIndex}
        totalSteps={TOTAL_STEPS}
        eyebrow="Get started"
        title="Welcome to AirNote."
        subtitle={
          isWindows
            ? "A two-minute setup. Connect your workspace, grant microphone access, add two free API keys, pick a hold-key — then you’ll never type by hand again."
            : "A two-minute setup. Connect your workspace, grant three permissions, add two free API keys, pick a hold-key — then you’ll never type by hand again."
        }
        brandTagline={
          isWindows
            ? "Voice polish for Windows. Hold a key, speak, release — AirNote types polished text into any app."
            : "Voice polish for Mac. Hold a key, speak, release — AirNote types polished text into any app."
        }
        brandKicker={isWindows ? "Built for Windows" : "Built for macOS"}
        brandQuote="It’s like typing, except your brain is the keyboard."
        bottomNote={<span>{isWindows ? "Windows 10/11" : "macOS 14+"}</span>}
      >
        <div className="mt-7 flex flex-col gap-2.5">
          <button onClick={goNext} className="btn-primary btn-lg w-full">
            Get started
            <ArrowRight size={14} />
          </button>
        </div>
      </OnboardingShell>
    );
  }

  // ── Step 2: Workspace connect ─────────────────────────────────────────────
  if (step === "workspace") {
    return (
      <OnboardingShell
        step={stepIndex}
        totalSteps={TOTAL_STEPS}
        eyebrow="Workspace"
        title={workspacePreview ? "You're connected." : "Connect your workspace."}
        subtitle={
          workspacePreview
            ? "Signed in to your organization. Continue setup on the next step."
            : "Enter your organization's server URL and sign in with email. Lark can be connected later."
        }
        brandTagline="Enterprise AirNote runs on your organization's server — your data stays in your workspace."
        brandKicker="Workspace sign-in"
        brandQuote="One workspace login, then every device knows who you are."
        topRight={<span>{stepLabel(step)}</span>}
        bottomNote={
          workspaceOnly
            ? <span>Reconnect to continue using AirNote</span>
            : <span>Managed by your organization</span>
        }
        onBack={workspaceOnly ? undefined : goBack}
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
          <div className="mt-7">
            <EnterpriseConnectForm
              compact
              variant="onboarding"
              onConnected={setWorkspacePreview}
              onCancel={workspaceOnly ? undefined : goBack}
            />
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
        totalSteps={TOTAL_STEPS}
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
        brandQuote="Audio never leaves the path between your mic and Deepgram. Nothing is stored on our servers."
        topRight={<span>{stepLabel(step)}</span>}
        bottomNote={<span>Change any time in Settings → Permissions</span>}
        onBack={goBack}
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
            onClick={goNext}
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

  // ── Step 4: API keys ─────────────────────────────────────────────────────
  if (step === "keys") {
    return (
      <OnboardingShell
        step={stepIndex}
        totalSteps={TOTAL_STEPS}
        eyebrow="Voice engine"
        title="Connect your voice."
        subtitle="Two free API keys do all the work. Both take under a minute."
        brandTagline="Two free keys. Speech-to-text and LLM polish — both on free tiers that cover daily use."
        brandKicker="Stored locally"
        brandQuote={`Your keys never leave this ${isWindows ? "PC" : "Mac"} except directly to Groq and Deepgram.`}
        topRight={<span>{stepLabel(step)}</span>}
        bottomNote={<span>Optional services (e.g. Gemini) live in Settings</span>}
        onBack={goBack}
      >
        <div className="mt-7 flex flex-col gap-3">
          <KeyCard
            color="#f55036"
            letter="G"
            name="Groq"
            href="https://console.groq.com/keys"
            placeholder="gsk_…"
            value={groqKey}
            onChange={setGroqKey}
            connected={!!prefs?.groq_api_key}
          />
          <KeyCard
            color="#0a8d8a"
            letter="D"
            name="Deepgram"
            href="https://console.deepgram.com/signup"
            placeholder="Paste your Deepgram key"
            value={deepgramKey}
            onChange={setDeepgramKey}
            connected={!!prefs?.deepgram_api_key}
          />

          {keyError && (
            <p className="text-[12px] text-center" style={{ color: "hsl(var(--destructive))" }}>
              {keyError}
            </p>
          )}

          <button
            onClick={handleSaveKeys}
            disabled={keySaving || !groqKey.trim() || !deepgramKey.trim()}
            className="btn-primary btn-lg w-full mt-1"
          >
            {keySaving ? "Saving…" : "Continue"}
            {!keySaving && <ArrowRight size={14} />}
          </button>
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

  return (
    <OnboardingShell
      step={stepIndex}
      totalSteps={TOTAL_STEPS}
      eyebrow="Hotkey"
      title="Pick a hold-key."
      subtitle="Hold this key to record, release to send. You can change it any time."
      brandTagline="Hold to record, release to send. Your thumb learns it in a day."
      brandKicker="Pro tip"
      brandQuote="Most users settle on Caps Lock — it’s already a hold-key for nothing useful."
      topRight={<span>{stepLabel(step)}</span>}
      bottomNote={<span>Press Caps Lock anywhere to dictate, once setup is done</span>}
      onBack={goBack}
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

// ── API key card ────────────────────────────────────────────────────────────

function KeyCard({
  color, letter, name, href, placeholder, value, onChange, connected,
}: {
  color: string;
  letter: string;
  name: string;
  href: string;
  placeholder: string;
  value: string;
  onChange: (v: string) => void;
  connected: boolean;
}) {
  return (
    <div
      className="rounded-lg p-3"
      style={{
        background: "hsl(0 0% 100% / 0.025)",
        boxShadow: "inset 0 0 0 1px hsl(var(--glass-stroke))",
      }}
    >
      <div className="flex items-center justify-between mb-2">
        <div className="flex items-center gap-2">
          <span
            className="w-[18px] h-[18px] rounded-[5px] grid place-items-center text-[10px] font-bold"
            style={{ background: color, color: "white" }}
          >
            {letter}
          </span>
          <span className="text-[12.5px] font-semibold" style={{ color: "hsl(var(--foreground))" }}>
            {name}
          </span>
          {connected && (
            <span className="accent-pill" style={{ color: "hsl(140 65% 65%)", background: "hsl(140 65% 50% / 0.14)" }}>
              Connected
            </span>
          )}
        </div>
        <button
          onClick={() => void openExternal(href)}
          className="flex items-center gap-1 text-[11px] font-medium transition-colors"
          style={{ color: "hsl(var(--muted-foreground))" }}
          onMouseEnter={(e) => (e.currentTarget.style.color = "hsl(var(--foreground))")}
          onMouseLeave={(e) => (e.currentTarget.style.color = "hsl(var(--muted-foreground))")}
        >
          Get free key
          <ExternalLink size={10} />
        </button>
      </div>
      <KeyInput value={value} onChange={onChange} placeholder={placeholder} />
    </div>
  );
}
