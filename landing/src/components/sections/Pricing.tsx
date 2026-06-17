"use client";

import { Check } from "lucide-react";
import { Section, SectionTitle, SectionLede } from "@/components/ui/Section";
import { pricing } from "@/lib/content";
import { Button } from "@/components/ui/Button";
import { cn } from "@/lib/cn";

export function Pricing() {
  return (
    <Section id="pricing">
      <div className="max-w-3xl">
        <SectionTitle>{pricing.title}</SectionTitle>
        <SectionLede>{pricing.subtitle}</SectionLede>
      </div>

      <div className="mt-14 grid md:grid-cols-3 gap-4">
        {pricing.tiers.map((t) => (
          <div
            key={t.name}
            className={cn(
              "relative rounded-2xl p-7 flex flex-col",
              t.highlight
                ? "bg-ink-800 border border-accent/40 shadow-[0_0_0_1px_rgba(165,180,252,0.22),0_30px_60px_-20px_rgba(165,180,252,0.18)]"
                : "bg-ink-800 hairline",
            )}
          >
            {t.highlight && (
              <span className="absolute -top-3 left-7 inline-flex items-center rounded-full bg-accent px-3 py-1 text-[10px] uppercase tracking-wider text-ink-900 font-medium">
                Most popular
              </span>
            )}
            <div className="text-sm text-ink-200">{t.name}</div>
            <div className="mt-2 flex items-baseline gap-2">
              <span className="font-display text-5xl tracking-tightest text-ink-50">
                {t.price}
              </span>
              <span className="text-sm text-ink-300">{t.cadence}</span>
            </div>
            <p className="mt-3 text-sm text-ink-200 leading-relaxed">{t.desc}</p>

            <ul className="mt-6 space-y-3 flex-1">
              {t.features.map((f) => (
                <li key={f} className="flex items-start gap-2 text-sm text-ink-100">
                  <Check
                    className={cn(
                      "h-4 w-4 mt-0.5 shrink-0",
                      t.highlight ? "text-accent" : "text-ink-200",
                    )}
                    strokeWidth={2.25}
                  />
                  <span>{f}</span>
                </li>
              ))}
            </ul>

            <Button
              variant={t.highlight ? "primary" : "secondary"}
              size="lg"
              className="mt-7 w-full"
            >
              {t.cta}
            </Button>
          </div>
        ))}
      </div>
    </Section>
  );
}
