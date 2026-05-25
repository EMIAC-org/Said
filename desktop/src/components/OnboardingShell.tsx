import React from "react";
import { ArrowLeft } from "lucide-react";
import { BrandMark } from "@/components/BrandMark";

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
            {bottomNote ?? `Step ${step + 1} of ${totalSteps}`}
          </span>
        </div>
      </div>
    </div>
  );
}
