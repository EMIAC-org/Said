import React from "react";
import {
  ArrowRight,
  Bell,
  Check,
  Key,
  Loader2,
  Mic,
  MonitorUp,
  Shield,
  Sparkles,
} from "lucide-react";
import { BrandMark } from "@/components/BrandMark";
import type { AppSnapshot } from "@/types";
import type { NotifPermission } from "@/lib/invoke";

interface PermissionStep {
  id: string;
  label: string;
  description: string;
  granted: boolean;
  required?: boolean;
  icon: React.ReactNode;
  actionLabel: string;
  onAction: () => void;
}

interface Props {
  snapshot: AppSnapshot | null;
  openAIConnected: boolean;
  connectBusy: boolean;
  connectError: string;
  notifPerm: NotifPermission;
  notifBusy: boolean;
  onConnectOpenAI: () => void;
  onMicrophone: () => void;
  onAccessibility: () => void;
  onInputMonitoring: () => void;
  onNotifications: () => void;
  onScreenRecording: () => void;
  onFinish: () => void;
}

function StatusPill({ granted, required }: { granted: boolean; required?: boolean }) {
  if (granted) {
    return (
      <span
        className="inline-flex items-center gap-1 rounded-lg px-2.5 py-1 text-[11px] font-semibold"
        style={{ background: "hsl(var(--primary) / 0.13)", color: "hsl(var(--primary))" }}
      >
        <Check size={11} /> Granted
      </span>
    );
  }

  return (
    <span
      className="inline-flex items-center rounded-lg px-2.5 py-1 text-[11px] font-semibold"
      style={{
        background: required ? "hsl(38 80% 45% / 0.14)" : "hsl(var(--surface-4))",
        color: required ? "hsl(38 90% 68%)" : "hsl(var(--muted-foreground))",
      }}
    >
      {required ? "Required" : "Optional"}
    </span>
  );
}

export function OnboardingFlow({
  snapshot,
  openAIConnected,
  connectBusy,
  connectError,
  notifPerm,
  notifBusy,
  onConnectOpenAI,
  onMicrophone,
  onAccessibility,
  onInputMonitoring,
  onNotifications,
  onScreenRecording,
  onFinish,
}: Props) {
  const microphoneGranted = snapshot?.microphone_granted ?? false;
  const accessibilityGranted = snapshot?.accessibility_granted ?? false;
  const inputMonitoringGranted = snapshot?.input_monitoring_granted ?? false;
  const screenRecordingGranted = snapshot?.screen_recording_granted ?? false;
  const notificationsGranted = notifPerm === "granted";

  const coreReady = microphoneGranted && accessibilityGranted && inputMonitoringGranted;
  const steps: PermissionStep[] = [
    {
      id: "mic",
      label: "Microphone",
      description: "Record your voice for dictation.",
      granted: microphoneGranted,
      required: true,
      icon: <Mic size={16} />,
      actionLabel: "Allow",
      onAction: onMicrophone,
    },
    {
      id: "input",
      label: "Input Monitoring",
      description: "Listen for the recording hotkey and global shortcuts.",
      granted: inputMonitoringGranted,
      required: true,
      icon: <Key size={16} />,
      actionLabel: "Open Settings",
      onAction: onInputMonitoring,
    },
    {
      id: "accessibility",
      label: "Accessibility",
      description: "Paste polished text into the app you are typing in.",
      granted: accessibilityGranted,
      required: true,
      icon: <Shield size={16} />,
      actionLabel: "Open Settings",
      onAction: onAccessibility,
    },
    {
      id: "notifications",
      label: "Notifications",
      description: "Get learning and edit-review alerts.",
      granted: notificationsGranted,
      icon: <Bell size={16} />,
      actionLabel: notifPerm === "denied" ? "Open Settings" : "Allow",
      onAction: onNotifications,
    },
    {
      id: "screen",
      label: "Screen Recording",
      description: "Optional context awareness for future smarter dictation.",
      granted: screenRecordingGranted,
      icon: <MonitorUp size={16} />,
      actionLabel: "Allow",
      onAction: onScreenRecording,
    },
  ];

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
          background: "radial-gradient(circle, hsl(var(--primary) / 0.12) 0%, transparent 66%)",
        }}
      />

      <div
        className="relative w-full max-w-[520px] rounded-[20px] p-7"
        style={{
          background: "hsl(var(--surface-2))",
          boxShadow:
            "inset 0 1px 0 hsl(0 0% 100% / 0.06), 0 24px 70px hsl(220 60% 2% / 0.50)",
        }}
      >
        <div className="flex items-start justify-between gap-4 mb-6">
          <div className="flex items-center gap-3">
            <BrandMark size={42} idSuffix="onboarding" />
            <div>
              <h1 className="text-[22px] font-extrabold leading-tight text-foreground">
                Set up Said
              </h1>
              <p className="text-[12.5px] text-muted-foreground mt-1">
                Connect OpenAI first, then grant the Mac permissions Said needs.
              </p>
            </div>
          </div>
          <span
            className="inline-flex items-center gap-1.5 rounded-lg px-2.5 py-1 text-[11px] font-semibold"
            style={{ background: "hsl(var(--surface-4))", color: "hsl(var(--muted-foreground))" }}
          >
            <Sparkles size={11} /> Onboarding
          </span>
        </div>

        {!openAIConnected ? (
          <div className="space-y-4">
            <div
              className="rounded-xl px-4 py-4"
              style={{
                background: "hsl(var(--surface-1))",
                boxShadow: "inset 0 0 0 1px hsl(var(--surface-4))",
              }}
            >
              <p className="text-[13px] font-semibold text-foreground">1. Connect OpenAI</p>
              <p className="text-[12px] text-muted-foreground mt-1 leading-relaxed">
                Said uses your OpenAI account to polish dictation with the models already selected for the app.
              </p>
            </div>

            {connectError && (
              <div
                className="rounded-lg px-3 py-2 text-[12px] font-medium"
                style={{
                  background: "hsl(354 78% 60% / 0.10)",
                  color: "hsl(354 78% 75%)",
                  boxShadow: "inset 0 0 0 1px hsl(354 78% 60% / 0.25)",
                }}
              >
                {connectError}
              </div>
            )}

            <button
              onClick={onConnectOpenAI}
              disabled={connectBusy}
              className="btn-primary w-full justify-center py-2.5 rounded-lg"
              style={{ fontSize: 13 }}
            >
              {connectBusy ? (
                <>
                  <Loader2 size={14} className="animate-spin" />
                  Waiting for browser…
                </>
              ) : (
                <>
                  Connect OpenAI
                  <ArrowRight size={13} />
                </>
              )}
            </button>
            {connectBusy && (
              <p className="text-[11.5px] text-muted-foreground text-center">
                Finish in your browser. This screen updates automatically.
              </p>
            )}
          </div>
        ) : (
          <div className="space-y-4">
            <div
              className="rounded-xl overflow-hidden"
              style={{ boxShadow: "inset 0 0 0 1px hsl(var(--surface-4))" }}
            >
              {steps.map((step, index) => (
                <div key={step.id}>
                  <div className="flex items-center gap-4 px-4 py-3.5">
                    <div
                      className="w-9 h-9 rounded-xl flex items-center justify-center flex-shrink-0"
                      style={{
                        background: "hsl(var(--surface-4))",
                        color: step.granted ? "hsl(var(--primary))" : "hsl(var(--muted-foreground))",
                      }}
                    >
                      {step.icon}
                    </div>
                    <div className="flex-1 min-w-0">
                      <div className="flex items-center gap-2">
                        <p className="text-[13px] font-semibold text-foreground">{step.label}</p>
                        <StatusPill granted={step.granted} required={step.required} />
                      </div>
                      <p className="text-[12px] text-muted-foreground mt-0.5 leading-relaxed">
                        {step.description}
                      </p>
                    </div>
                    {!step.granted && (
                      <button
                        onClick={step.onAction}
                        disabled={step.id === "notifications" && notifBusy}
                        className="text-[12px] font-semibold px-3 py-1.5 rounded-lg transition-colors disabled:opacity-60"
                        style={{ background: "hsl(var(--primary))", color: "hsl(var(--primary-foreground))" }}
                      >
                        {step.id === "notifications" && notifBusy ? "Opening…" : step.actionLabel}
                      </button>
                    )}
                  </div>
                  {index < steps.length - 1 && (
                    <div className="mx-4 border-t" style={{ borderColor: "hsl(var(--surface-4))" }} />
                  )}
                </div>
              ))}
            </div>

            <button
              onClick={onFinish}
              disabled={!coreReady}
              className="btn-primary w-full justify-center py-2.5 rounded-lg disabled:opacity-50"
              style={{ fontSize: 13 }}
            >
              Continue to Said
              <ArrowRight size={13} />
            </button>
            {!coreReady && (
              <p className="text-[11.5px] text-muted-foreground text-center leading-relaxed">
                Microphone, Input Monitoring, and Accessibility are required for the full dictation flow.
              </p>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
