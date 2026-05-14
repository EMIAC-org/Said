import React, { useCallback, useEffect, useState } from "react";
import {
  ArrowRight,
  Check,
  ExternalLink,
  Eye,
  EyeOff,
  Key,
  Keyboard,
  Mic,
  Shield,
  Sparkles,
} from "lucide-react";
import { BrandMark } from "@/components/BrandMark";
import type { AppSnapshot, Preferences } from "@/types";
import { getPreferences, patchPreferences } from "@/lib/invoke";

type Step = "welcome" | "microphone" | "accessibility" | "api-keys" | "hotkey" | "input-monitoring" | "test";

interface Props {
  snapshot: AppSnapshot | null;
  onMicrophone: () => void;
  onAccessibility: () => void;
  onInputMonitoring: () => void;
  onFinish: () => void;
}

const STEPS: Step[] = ["welcome", "microphone", "accessibility", "api-keys", "hotkey", "input-monitoring", "test"];

function ProgressDots({ current }: { current: number }) {
  return (
    <div className="flex gap-1.5 justify-center">
      {STEPS.map((_, i) => (
        <div
          key={i}
          className="h-1.5 rounded-full transition-all duration-300"
          style={{
            width: i === current ? 24 : 8,
            background: i <= current ? "hsl(var(--primary))" : "hsl(var(--surface-4))",
          }}
        />
      ))}
    </div>
  );
}

function StepShell({
  icon,
  title,
  subtitle,
  children,
  step,
}: {
  icon: React.ReactNode;
  title: string;
  subtitle: string;
  children: React.ReactNode;
  step: number;
}) {
  return (
    <div
      className="flex h-screen w-screen items-center justify-center overflow-hidden relative"
      style={{ background: "hsl(var(--background))" }}
    >
      <div aria-hidden data-tauri-drag-region className="absolute inset-x-0 top-0 h-12 drag-region" />
      <div
        aria-hidden
        className="absolute pointer-events-none"
        style={{
          top: "-18%",
          left: "50%",
          transform: "translateX(-50%)",
          width: 720,
          height: 720,
          borderRadius: "50%",
          background: "radial-gradient(circle, hsl(var(--primary) / 0.10) 0%, transparent 66%)",
        }}
      />
      <div
        className="relative w-full max-w-[480px] rounded-[20px] p-7"
        style={{
          background: "hsl(var(--surface-2))",
          boxShadow: "inset 0 1px 0 hsl(0 0% 100% / 0.06), 0 24px 70px hsl(220 60% 2% / 0.50)",
        }}
      >
        <div className="flex flex-col items-center text-center mb-6">
          <div
            className="w-14 h-14 rounded-2xl flex items-center justify-center mb-4"
            style={{ background: "hsl(var(--primary) / 0.12)", color: "hsl(var(--primary))" }}
          >
            {icon}
          </div>
          <h1 className="text-[20px] font-bold text-foreground leading-tight">{title}</h1>
          <p className="text-[13px] text-muted-foreground mt-1.5 max-w-[360px] leading-relaxed">{subtitle}</p>
        </div>
        {children}
        <div className="mt-6">
          <ProgressDots current={step} />
        </div>
      </div>
    </div>
  );
}

function PasswordInput({
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
        className="w-full rounded-xl px-4 py-3 text-[13px] text-foreground outline-none pr-10"
        style={{
          background: "hsl(var(--surface-1))",
          boxShadow: "inset 0 0 0 1px hsl(var(--surface-4))",
        }}
      />
      <button
        type="button"
        onClick={() => setShow((s) => !s)}
        className="absolute right-3 top-1/2 -translate-y-1/2 text-muted-foreground"
      >
        {show ? <EyeOff size={14} /> : <Eye size={14} />}
      </button>
    </div>
  );
}

function computeStartStep(
  mic: boolean, acc: boolean, im: boolean, hasKeys: boolean,
): Step {
  if (!mic) return "microphone";
  if (!acc) return "accessibility";
  if (!hasKeys) return "api-keys";
  if (!im) return "input-monitoring";
  return "test";
}

export function OnboardingFlow({ snapshot, onMicrophone, onAccessibility, onInputMonitoring, onFinish }: Props) {
  const [prefs, setPrefs] = useState<Preferences | null>(null);
  const [prefsLoaded, setPrefsLoaded] = useState(false);
  const [groqKey, setGroqKey] = useState("");
  const [deepgramKey, setDeepgramKey] = useState("");
  const [keySaving, setKeySaving] = useState(false);
  const [keyError, setKeyError] = useState("");

  const micGranted = snapshot?.microphone_granted ?? false;
  const accGranted = snapshot?.accessibility_granted ?? false;
  const imGranted = snapshot?.input_monitoring_granted ?? false;

  useEffect(() => {
    getPreferences().then((p) => {
      if (p) {
        setPrefs(p);
        if (p.groq_api_key) setGroqKey(p.groq_api_key);
        if (p.deepgram_api_key) setDeepgramKey(p.deepgram_api_key);
      }
      setPrefsLoaded(true);
    });
  }, []);

  const hasKeys = !!(prefs?.groq_api_key && prefs?.deepgram_api_key);
  const startStep = prefsLoaded
    ? computeStartStep(micGranted, accGranted, imGranted, hasKeys)
    : "welcome";

  const [step, setStep] = useState<Step>("welcome");
  const [initialized, setInitialized] = useState(false);

  useEffect(() => {
    if (prefsLoaded && !initialized) {
      setStep(startStep === "test" ? "test" : startStep);
      setInitialized(true);
    }
  }, [prefsLoaded, initialized, startStep]);

  const stepIndex = STEPS.indexOf(step);

  const goNext = useCallback(() => {
    const idx = STEPS.indexOf(step);
    if (idx < STEPS.length - 1) setStep(STEPS[idx + 1]);
  }, [step]);

  // Auto-advance when permission is granted or keys are already saved
  useEffect(() => {
    if (step === "microphone" && micGranted) {
      const t = setTimeout(goNext, 600);
      return () => clearTimeout(t);
    }
    if (step === "accessibility" && accGranted) {
      const t = setTimeout(goNext, 600);
      return () => clearTimeout(t);
    }
    if (step === "api-keys" && hasKeys) {
      const t = setTimeout(goNext, 300);
      return () => clearTimeout(t);
    }
    if (step === "input-monitoring" && imGranted) {
      const t = setTimeout(goNext, 600);
      return () => clearTimeout(t);
    }
  }, [step, micGranted, accGranted, imGranted, hasKeys, goNext]);

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
    goNext();
  }, [goNext]);

  // ── Step 1: Welcome ──────────────────────────────────────────────────────
  if (step === "welcome") {
    return (
      <StepShell
        icon={<BrandMark size={28} idSuffix="onboarding" />}
        title="Welcome to Said"
        subtitle="Hold a key, speak, release — Said types polished text into any app."
        step={stepIndex}
      >
        <button
          onClick={goNext}
          className="btn-primary w-full justify-center py-3 rounded-xl text-[14px] font-semibold"
        >
          Get Started
          <ArrowRight size={15} />
        </button>
      </StepShell>
    );
  }

  // ── Step 2: Microphone ───────────────────────────────────────────────────
  if (step === "microphone") {
    return (
      <StepShell
        icon={<Mic size={24} />}
        title="Microphone Access"
        subtitle="Said needs your microphone to hear what you say."
        step={stepIndex}
      >
        {micGranted ? (
          <div className="flex items-center justify-center gap-2 py-3 text-[14px] font-semibold" style={{ color: "hsl(var(--primary))" }}>
            <Check size={18} /> Microphone allowed
          </div>
        ) : (
          <button
            onClick={onMicrophone}
            className="btn-primary w-full justify-center py-3 rounded-xl text-[14px] font-semibold"
          >
            Allow Microphone
          </button>
        )}
      </StepShell>
    );
  }

  // ── Step 3: Accessibility ────────────────────────────────────────────────
  if (step === "accessibility") {
    return (
      <StepShell
        icon={<Shield size={24} />}
        title="Accessibility"
        subtitle="Said types polished text directly into the app you're using."
        step={stepIndex}
      >
        {accGranted ? (
          <div className="flex items-center justify-center gap-2 py-3 text-[14px] font-semibold" style={{ color: "hsl(var(--primary))" }}>
            <Check size={18} /> Accessibility granted
          </div>
        ) : (
          <>
            <button
              onClick={onAccessibility}
              className="btn-primary w-full justify-center py-3 rounded-xl text-[14px] font-semibold"
            >
              Open Settings
              <ArrowRight size={15} />
            </button>
            <p className="text-[11.5px] text-muted-foreground text-center mt-2">
              Add Said in System Settings → Privacy → Accessibility, then come back.
            </p>
          </>
        )}
      </StepShell>
    );
  }

  // ── Step 4: API Keys ─────────────────────────────────────────────────────
  if (step === "api-keys") {
    return (
      <StepShell
        icon={<Key size={24} />}
        title="Connect Your API Keys"
        subtitle="Said needs two free API keys to work. Both take under a minute to get."
        step={stepIndex}
      >
        <div className="space-y-4">
          {/* Groq Section */}
          <div
            className="rounded-xl px-4 py-4"
            style={{ background: "hsl(var(--surface-1))", boxShadow: "inset 0 0 0 1px hsl(var(--surface-4))" }}
          >
            <div className="flex items-center justify-between mb-2">
              <span className="text-[13px] font-semibold text-foreground">Groq</span>
              {prefs?.groq_api_key ? (
                <span className="text-[10px] font-semibold flex items-center gap-1 px-2 py-0.5 rounded-md" style={{ background: "hsl(var(--primary) / 0.12)", color: "hsl(var(--primary))" }}>
                  <Check size={10} /> Connected
                </span>
              ) : (
                <span className="text-[10px] font-medium px-2 py-0.5 rounded-md" style={{ background: "hsl(38 80% 45% / 0.14)", color: "hsl(38 90% 68%)" }}>
                  Required
                </span>
              )}
            </div>
            <p className="text-[11.5px] text-muted-foreground mb-2.5 leading-relaxed">
              Polishes your speech into clean text using fast LLM inference.
            </p>
            <div className="flex items-center gap-2 mb-2.5">
              <a
                href="https://console.groq.com/keys"
                target="_blank"
                rel="noreferrer"
                className="inline-flex items-center gap-1.5 text-[11.5px] font-semibold px-3 py-1.5 rounded-lg transition-colors"
                style={{ background: "hsl(var(--surface-4))", color: "hsl(var(--foreground))" }}
              >
                Get free key <ExternalLink size={11} />
              </a>
              <span className="text-[10px] text-muted-foreground">console.groq.com</span>
            </div>
            <ul className="text-[11px] text-muted-foreground space-y-1 mb-3 ml-3">
              <li className="flex items-start gap-1.5"><span style={{ color: "hsl(var(--primary))" }}>1.</span> Sign up or log in with Google</li>
              <li className="flex items-start gap-1.5"><span style={{ color: "hsl(var(--primary))" }}>2.</span> Click "Create API Key"</li>
              <li className="flex items-start gap-1.5"><span style={{ color: "hsl(var(--primary))" }}>3.</span> Copy the key (starts with <span className="font-mono">gsk_</span>)</li>
            </ul>
            <PasswordInput value={groqKey} onChange={setGroqKey} placeholder="gsk_..." />
          </div>

          {/* Deepgram Section */}
          <div
            className="rounded-xl px-4 py-4"
            style={{ background: "hsl(var(--surface-1))", boxShadow: "inset 0 0 0 1px hsl(var(--surface-4))" }}
          >
            <div className="flex items-center justify-between mb-2">
              <span className="text-[13px] font-semibold text-foreground">Deepgram</span>
              {prefs?.deepgram_api_key ? (
                <span className="text-[10px] font-semibold flex items-center gap-1 px-2 py-0.5 rounded-md" style={{ background: "hsl(var(--primary) / 0.12)", color: "hsl(var(--primary))" }}>
                  <Check size={10} /> Connected
                </span>
              ) : (
                <span className="text-[10px] font-medium px-2 py-0.5 rounded-md" style={{ background: "hsl(38 80% 45% / 0.14)", color: "hsl(38 90% 68%)" }}>
                  Required
                </span>
              )}
            </div>
            <p className="text-[11.5px] text-muted-foreground mb-2.5 leading-relaxed">
              Converts your voice to text with real-time speech recognition.
            </p>
            <div className="flex items-center gap-2 mb-2.5">
              <a
                href="https://console.deepgram.com/signup"
                target="_blank"
                rel="noreferrer"
                className="inline-flex items-center gap-1.5 text-[11.5px] font-semibold px-3 py-1.5 rounded-lg transition-colors"
                style={{ background: "hsl(var(--surface-4))", color: "hsl(var(--foreground))" }}
              >
                Get free key <ExternalLink size={11} />
              </a>
              <span className="text-[10px] text-muted-foreground">console.deepgram.com</span>
            </div>
            <ul className="text-[11px] text-muted-foreground space-y-1 mb-3 ml-3">
              <li className="flex items-start gap-1.5"><span style={{ color: "hsl(var(--primary))" }}>1.</span> Sign up with Google or GitHub</li>
              <li className="flex items-start gap-1.5"><span style={{ color: "hsl(var(--primary))" }}>2.</span> Go to Settings → API Keys</li>
              <li className="flex items-start gap-1.5"><span style={{ color: "hsl(var(--primary))" }}>3.</span> Create a key and copy it</li>
            </ul>
            <PasswordInput value={deepgramKey} onChange={setDeepgramKey} placeholder="Your Deepgram key" />
          </div>

          {keyError && (
            <p className="text-[12px] font-medium text-center" style={{ color: "hsl(354 78% 65%)" }}>
              {keyError}
            </p>
          )}

          <button
            onClick={handleSaveKeys}
            disabled={keySaving || !groqKey.trim() || !deepgramKey.trim()}
            className="btn-primary w-full justify-center py-3 rounded-xl text-[14px] font-semibold disabled:opacity-50"
          >
            {keySaving ? "Saving…" : "Save & Continue"}
            {!keySaving && <ArrowRight size={15} />}
          </button>

          <p className="text-[10.5px] text-muted-foreground text-center leading-relaxed">
            Keys are stored locally on your device. Never sent anywhere except directly to Groq and Deepgram.
          </p>
        </div>
      </StepShell>
    );
  }

  // ── Step 5: Choose Hotkey ────────────────────────────────────────────────
  if (step === "hotkey") {
    const options = [
      { key: "caps_lock", label: "Caps Lock", desc: "Quick single key" },
      { key: "right_option", label: "Right Option", desc: "Hold to record" },
      { key: "fn", label: "Fn", desc: "Hold to record" },
    ];

    return (
      <StepShell
        icon={<Keyboard size={24} />}
        title="Choose Your Hotkey"
        subtitle="Pick which key you'll hold to start recording."
        step={stepIndex}
      >
        <div className="space-y-2">
          {options.map((opt) => (
            <button
              key={opt.key}
              onClick={() => handleHotkeySelect(opt.key)}
              className="w-full flex items-center gap-4 px-4 py-3.5 rounded-xl transition-colors text-left"
              style={{
                background: "hsl(var(--surface-1))",
                boxShadow: "inset 0 0 0 1px hsl(var(--surface-4))",
              }}
              onMouseEnter={(e) => { e.currentTarget.style.background = "hsl(var(--surface-4))"; }}
              onMouseLeave={(e) => { e.currentTarget.style.background = "hsl(var(--surface-1))"; }}
            >
              <div
                className="w-10 h-10 rounded-xl flex items-center justify-center flex-shrink-0 text-[13px] font-bold"
                style={{ background: "hsl(var(--primary) / 0.12)", color: "hsl(var(--primary))" }}
              >
                {opt.key === "caps_lock" ? "⇪" : opt.key === "fn" ? "fn" : "⌥"}
              </div>
              <div>
                <p className="text-[14px] font-semibold text-foreground">{opt.label}</p>
                <p className="text-[12px] text-muted-foreground">{opt.desc}</p>
              </div>
              <ArrowRight size={14} className="ml-auto text-muted-foreground" />
            </button>
          ))}
        </div>
      </StepShell>
    );
  }

  // ── Step 6: Input Monitoring ─────────────────────────────────────────────
  if (step === "input-monitoring") {
    return (
      <StepShell
        icon={<Sparkles size={24} />}
        title="Input Monitoring"
        subtitle="Last permission — lets Said listen for your hotkey from any app."
        step={stepIndex}
      >
        {imGranted ? (
          <div className="flex items-center justify-center gap-2 py-3 text-[14px] font-semibold" style={{ color: "hsl(var(--primary))" }}>
            <Check size={18} /> Input Monitoring granted
          </div>
        ) : (
          <>
            <button
              onClick={onInputMonitoring}
              className="btn-primary w-full justify-center py-3 rounded-xl text-[14px] font-semibold"
            >
              Open Settings
              <ArrowRight size={15} />
            </button>
            <p className="text-[11.5px] text-muted-foreground text-center mt-2">
              Add Said in System Settings → Privacy → Input Monitoring, then come back.
            </p>
          </>
        )}
      </StepShell>
    );
  }

  // ── Step 7: Test It ──────────────────────────────────────────────────────
  const hotkeyLabel = prefs?.record_hotkey === "fn" ? "Fn" : prefs?.record_hotkey === "right_option" ? "Right Option" : "Caps Lock";

  return (
    <StepShell
      icon={<Mic size={24} />}
      title="You're All Set!"
      subtitle={`Press and hold ${hotkeyLabel}, say something, then release. Try it now!`}
      step={stepIndex}
    >
      <div
        className="rounded-xl px-5 py-4 mb-4 text-center"
        style={{
          background: "hsl(var(--surface-1))",
          boxShadow: "inset 0 0 0 1px hsl(var(--surface-4))",
        }}
      >
        <p className="text-[13px] text-muted-foreground leading-relaxed">
          Hold <span className="font-bold text-foreground">{hotkeyLabel}</span> → speak → release.
          Said will type polished text into whatever app is focused.
        </p>
      </div>
      <button
        onClick={onFinish}
        className="btn-primary w-full justify-center py-3 rounded-xl text-[14px] font-semibold"
      >
        Start Using Said
        <ArrowRight size={15} />
      </button>
    </StepShell>
  );
}
