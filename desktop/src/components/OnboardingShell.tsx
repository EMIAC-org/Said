import React from "react";
import { ArrowLeft } from "lucide-react";
import { BrandMark } from "@/components/BrandMark";
import { BrandWave } from "@/components/BrandWave";

export interface OnboardingStepNavItem {
  id: string;
  label: string;
  index: number;
}

export function OnboardingShell({
  step,
  totalSteps,
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
  totalSteps: number;
  eyebrow: string;
  title: string;
  subtitle: string;
  brandTagline: string;
  brandKicker: string;
  brandQuote: string;
  topRight?: React.ReactNode;
  bottomNote?: React.ReactNode;
  onBack?: () => void;
  steps?: OnboardingStepNavItem[];
  currentStepIndex?: number;
  maxReachableIndex?: number;
  stepStatus?: Record<string, "pending" | "done" | "current">;
  onStepSelect?: (index: number) => void;
  children: React.ReactNode;
}) {
  return (
    <div
      className="onb-split relative"
      style={{ background: "hsl(var(--background))" }}
    >
      <div
        aria-hidden
        data-tauri-drag-region
        className="absolute inset-x-0 top-0 h-7 drag-region z-10"
      />

      <div className="onb-brand">
        <div className="onb-brand-lockup">
          <span className="mk"><BrandMark size={15} /></span>
          AirNote
        </div>

        <div className="onb-brand-center">
          <BrandWave />
          <p className="onb-brand-headline">{brandTagline}</p>
        </div>

        <div className="onb-brand-foot">
          <span className="onb-brand-kicker">{brandKicker}</span>
          <p className="onb-brand-quote">“{brandQuote}”</p>
        </div>
      </div>

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

        <div className="flex flex-col gap-2 flex-shrink-0">
          <div className="flex items-center justify-end" style={{ minHeight: 24 }}>
            <span className="text-[11px]" style={{ color: "hsl(var(--muted-foreground) / 0.6)" }}>
              {bottomNote ?? `Step ${step + 1} of ${totalSteps}`}
            </span>
          </div>
        </div>
      </div>
    </div>
  );
}
