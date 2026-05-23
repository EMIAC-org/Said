import React, { useCallback, useEffect, useState } from "react";
import {
  ArrowLeft,
  ArrowRight,
  Check,
  ExternalLink,
  Eye,
  EyeOff,
  Mic,
  Shield,
  Keyboard,
} from "lucide-react";
import { BrandMark } from "@/components/BrandMark";
import type { AppSnapshot, Preferences } from "@/types";
import { getPreferences, patchPreferences, openExternal } from "@/lib/invoke";

type Step = "welcome" | "permissions" | "keys" | "hotkey";

interface Props {
  snapshot: AppSnapshot | null;
  onMicrophone: () => void;
  onAccessibility: () => void;
  onInputMonitoring: () => void;
  onFinish: () => void;
}

const STEPS: Step[] = ["welcome", "permissions", "keys", "hotkey"];

// ── Layout primitive: the 50/50 split with brand canvas + form ──────────────

function SplitShell({
  step,
  eyebrow,
  title,
  subtitle,
  brandTagline,
  brandKicker,
  brandQuote,
  topRight,
  bottomNote,
  onBack,
  children,
}: {
  step: number;
  eyebrow: string;
  title: string;
  subtitle: string;
  brandTagline: string;
  brandKicker: string;
  brandQuote: string;
  topRight?: React.ReactNode;
  bottomNote?: React.ReactNode;
  onBack?: () => void;
  children: React.ReactNode;
}) {
  return (
    <div
      className="onb-split relative"
      style={{ background: "hsl(var(--background))" }}
    >
      {/* Drag region — top strip across the whole window */}
      <div
        aria-hidden
        data-tauri-drag-region
        className="absolute inset-x-0 top-0 h-7 drag-region z-10"
      />

      {/* ── LEFT: brand canvas ─────────────────────────────────────────── */}
      <div className="onb-brand">
        <div className="relative z-10 flex items-center gap-2 text-[12.5px] font-medium" style={{ color: "hsl(var(--foreground))" }}>
          <span style={{ width: 18, height: 18, display: "inline-grid", placeItems: "center", color: "hsl(var(--foreground))" }}>
            <BrandMark size={18} />
          </span>
          AirNote
        </div>

        <div className="relative z-10 flex-1 flex flex-col items-center justify-center gap-6 text-center">
          <span className="onb-brand-mark">
            <BrandMark size={64} />
          </span>
          <div>
            <div
              style={{
                fontSize: 44,
                fontWeight: 500,
                letterSpacing: "-0.035em",
                lineHeight: 1,
                color: "hsl(var(--foreground))",
              }}
            >
              AirNote
            </div>
            <p
              style={{
                fontSize: 13.5,
                color: "hsl(var(--muted-foreground))",
                lineHeight: 1.55,
                maxWidth: 280,
                margin: "12px auto 0",
              }}
            >
              {brandTagline}
            </p>
          </div>
        </div>

        <div
          className="relative z-10 pt-4"
          style={{
            borderTop: "1px solid hsl(var(--border))",
            color: "hsl(var(--muted-foreground) / 0.85)",
          }}
        >
          <span
            className="block text-[10.5px] font-semibold uppercase tracking-[0.14em] mb-1.5"
            style={{ color: "hsl(var(--muted-foreground))" }}
          >
            {brandKicker}
          </span>
          <p
            className="text-[12.5px] italic leading-relaxed"
            style={{ color: "hsl(var(--foreground) / 0.85)" }}
          >
            “{brandQuote}”
          </p>
        </div>
      </div>

      {/* ── RIGHT: form canvas ─────────────────────────────────────────── */}
      <div className="onb-form">
        <div className="flex items-center justify-between flex-shrink-0" style={{ minHeight: 36 }}>
          {onBack ? (
            <button
              onClick={onBack}
              className="no-drag flex items-center justify-center transition-colors"
              style={{
                width: 30,
                height: 30,
                borderRadius: 8,
                color: "hsl(var(--muted-foreground))",
                border: "1px solid transparent",
                background: "transparent",
              }}
              onMouseEnter={(e) => {
                e.currentTarget.style.color = "hsl(var(--foreground))";
                e.currentTarget.style.background = "hsl(0 0% 100% / 0.04)";
                e.currentTarget.style.borderColor = "hsl(var(--glass-stroke-strong))";
              }}
              onMouseLeave={(e) => {
                e.currentTarget.style.color = "hsl(var(--muted-foreground))";
                e.currentTarget.style.background = "transparent";
                e.currentTarget.style.borderColor = "transparent";
              }}
              aria-label="Back"
            >
              <ArrowLeft size={14} />
            </button>
          ) : (
            <span />
          )}
          <div className="text-[11.5px]" style={{ color: "hsl(var(--muted-foreground) / 0.7)" }}>
            {topRight ?? null}
          </div>
        </div>

        <div className="flex-1 flex flex-col justify-center" style={{ maxWidth: 440, width: "100%", margin: "0 auto", padding: "24px 0" }}>
          <p
            className="text-[10.5px] font-semibold uppercase tracking-[0.16em] mb-3"
            style={{ color: "hsl(var(--primary))" }}
          >
            {eyebrow}
          </p>
          <h1
            className="m-0"
            style={{
              fontSize: 28,
              fontWeight: 600,
              letterSpacing: "-0.025em",
              lineHeight: 1.18,
              color: "hsl(var(--foreground))",
            }}
          >
            {title}
          </h1>
          <p
            className="mt-3 mb-0"
            style={{
              fontSize: 13.5,
              color: "hsl(var(--muted-foreground))",
              lineHeight: 1.6,
            }}
          >
            {subtitle}
          </p>

          {children}
        </div>

        <div className="flex items-center justify-end flex-shrink-0" style={{ minHeight: 24 }}>
          <span className="text-[11px]" style={{ color: "hsl(var(--muted-foreground) / 0.6)" }}>
            {bottomNote ?? `Step ${step + 1} of ${STEPS.length}`}
          </span>
        </div>
      </div>
    </div>
  );
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

// ── Step picker — what step does the user land on? ─────────────────────────

function computeStartStep(
  mic: boolean, acc: boolean, im: boolean, hasKeys: boolean,
): Step {
  if (!mic || !acc || !im) return "permissions";
  if (!hasKeys) return "keys";
  return "hotkey";
}

// ── Main component ─────────────────────────────────────────────────────────

export function OnboardingFlow({
  snapshot,
  onMicrophone,
  onAccessibility,
  onInputMonitoring,
  onFinish,
}: Props) {
  const [prefs, setPrefs] = useState<Preferences | null>(null);
  const [prefsLoaded, setPrefsLoaded] = useState(false);
  const [groqKey, setGroqKey] = useState("");
  const [deepgramKey, setDeepgramKey] = useState("");
  const [keySaving, setKeySaving] = useState(false);
  const [keyError, setKeyError] = useState("");

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
      setStep(startStep);
      setInitialized(true);
    }
  }, [prefsLoaded, initialized, startStep]);

  const stepIndex = STEPS.indexOf(step);

  const goNext = useCallback(() => {
    const idx = STEPS.indexOf(step);
    if (idx < STEPS.length - 1) setStep(STEPS[idx + 1]);
  }, [step]);

  const goBack = useCallback(() => {
    const idx = STEPS.indexOf(step);
    if (idx > 0) setStep(STEPS[idx - 1]);
  }, [step]);

  // Auto-advance the permissions step once all three are granted (delay so
  // the user sees the green check before the screen swaps).
  useEffect(() => {
    if (step === "permissions" && micGranted && accGranted && imGranted) {
      const t = setTimeout(goNext, 700);
      return () => clearTimeout(t);
    }
    if (step === "keys" && hasKeys) {
      const t = setTimeout(goNext, 300);
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
    onFinish();
  }, [onFinish]);

  // ── Step 1: Welcome ──────────────────────────────────────────────────────
  if (step === "welcome") {
    return (
      <SplitShell
        step={stepIndex}
        eyebrow="Get started"
        title="Welcome to AirNote."
        subtitle="A two-minute setup. Three permissions, two free API keys, one hold-key. Then you’ll never type by hand again."
        brandTagline="Voice polish for Mac. Hold a key, speak, release — AirNote types polished text into any app."
        brandKicker="Built for macOS"
        brandQuote="It’s like typing, except your brain is the keyboard."
        bottomNote={<span>v2.0.3 · macOS 14+</span>}
      >
        <div className="mt-7 flex flex-col gap-2.5">
          <button onClick={goNext} className="btn-primary btn-lg w-full">
            Get started
            <ArrowRight size={14} />
          </button>
        </div>
      </SplitShell>
    );
  }

  // ── Step 2: Permissions ──────────────────────────────────────────────────
  if (step === "permissions") {
    const allGranted = micGranted && accGranted && imGranted;
    return (
      <SplitShell
        step={stepIndex}
        eyebrow="Permissions"
        title="A few system grants."
        subtitle="Three macOS permissions — one click each. AirNote detects each grant automatically."
        brandTagline="macOS will ask you once for each permission. AirNote detects each grant the moment it happens."
        brandKicker="Privacy"
        brandQuote="Audio never leaves the path between your mic and Deepgram. Nothing is stored on our servers."
        topRight={<span>2 of 4</span>}
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
          <PermRow
            icon={<Shield size={15} />}
            title="Accessibility"
            desc="Type polished text into the focused app."
            granted={accGranted}
            onAllow={onAccessibility}
          />
          <PermRow
            icon={<Keyboard size={15} />}
            title="Input Monitoring"
            desc="Hear your hotkey from any app — even when AirNote isn’t focused."
            granted={imGranted}
            onAllow={onInputMonitoring}
          />
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
      </SplitShell>
    );
  }

  // ── Step 3: API keys ─────────────────────────────────────────────────────
  if (step === "keys") {
    return (
      <SplitShell
        step={stepIndex}
        eyebrow="Voice engine"
        title="Connect your voice."
        subtitle="Two free API keys do all the work. Both take under a minute."
        brandTagline="Two free keys. Speech-to-text and LLM polish — both on free tiers that cover daily use."
        brandKicker="Stored locally"
        brandQuote="Your keys never leave this Mac except directly to Groq and Deepgram."
        topRight={<span>3 of 4</span>}
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
      </SplitShell>
    );
  }

  // ── Step 4: Hotkey ───────────────────────────────────────────────────────
  const currentHotkey = prefs?.record_hotkey ?? "caps_lock";
  const options: { key: string; glyph: string; label: string; desc: string }[] = [
    { key: "caps_lock",    glyph: "⇪",  label: "Caps Lock",     desc: "Single key, easy to hold." },
    { key: "right_option", glyph: isWindows ? "Alt" : "⌥",  label: isWindows ? "Right Alt" : "Right Option",  desc: "Stays out of the way." },
    ...(!isWindows
      ? [{ key: "fn", glyph: "fn", label: "Fn / Globe", desc: "The world key on MacBooks." }]
      : []),
  ];

  return (
    <SplitShell
      step={stepIndex}
      eyebrow="Hotkey"
      title="Pick a hold-key."
      subtitle="Hold this key to record, release to send. You can change it any time."
      brandTagline="Hold to record, release to send. Your thumb learns it in a day."
      brandKicker="Pro tip"
      brandQuote="Most users settle on Caps Lock — it’s already a hold-key for nothing useful."
      topRight={<span>4 of 4</span>}
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
    </SplitShell>
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
