"use client";

import { motion, useReducedMotion } from "framer-motion";
import { Section } from "@/components/ui/Section";
import { mobile } from "@/lib/content";
import { ButtonLink } from "@/components/ui/Button";

function PhoneFrame() {
  const reduce = useReducedMotion();
  return (
    <div className="relative mx-auto w-[280px]">
      <div className="rounded-[44px] bg-ink-700 hairline-strong p-3 shadow-2xl shadow-black/50">
        <div className="rounded-[32px] bg-ink-900 aspect-[9/19] relative overflow-hidden">
          <div className="absolute top-3 left-1/2 -translate-x-1/2 h-6 w-24 bg-black rounded-full" />
          <div className="absolute inset-0 p-6 pt-16 flex flex-col">
            <div className="text-xs text-ink-300 uppercase tracking-wider">12:04</div>
            <div className="mt-8 space-y-3">
              <div className="rounded-2xl bg-ink-800 hairline p-4">
                <div className="text-[10px] uppercase tracking-wider text-accent mb-1">
                  Just dictated
                </div>
                <div className="text-sm text-ink-50 leading-relaxed">
                  Bring the offsite agenda forward by a week — most of the team is back from leave.
                </div>
              </div>
              <div className="rounded-2xl bg-ink-800/60 hairline p-4">
                <div className="text-xs text-ink-300">Synced to Mac</div>
                <div className="mt-1 flex items-center gap-2">
                  <span className="h-1.5 w-1.5 rounded-full bg-green-400" />
                  <span className="text-xs text-ink-100">Pasted in Notion</span>
                </div>
              </div>
            </div>
            <div className="mt-auto flex justify-center pb-4">
              <motion.button
                aria-label="Press and hold to dictate"
                animate={reduce ? undefined : { scale: [1, 1.06, 1] }}
                transition={
                  reduce
                    ? undefined
                    : { duration: 2, repeat: Infinity, ease: "easeInOut" }
                }
                className="h-16 w-16 rounded-full bg-accent text-ink-900 grid place-items-center font-medium shadow-lg shadow-accent/30"
              >
                Hold
              </motion.button>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

function QRCode() {
  return (
    <div
      aria-label="QR code placeholder for App Store"
      className="h-28 w-28 rounded-lg bg-ink-50 p-2 grid grid-cols-12 grid-rows-12 gap-px"
    >
      {Array.from({ length: 144 }).map((_, i) => {
        const seed = (i * 9301 + 49297) % 233280;
        const on = seed / 233280 > 0.55;
        const corner =
          (i < 36 && i % 12 < 3) ||
          (i % 12 > 8 && i < 36) ||
          (i >= 108 && i % 12 < 3);
        return (
          <span
            key={i}
            className={on || corner ? "bg-ink-900 rounded-[1px]" : "bg-transparent"}
          />
        );
      })}
    </div>
  );
}

export function MobileSection() {
  return (
    <Section id="mobile">
      <div className="grid lg:grid-cols-2 gap-12 lg:gap-20 items-center">
        <div>
          <p className="text-xs uppercase tracking-[0.2em] text-accent mb-4">
            {mobile.eyebrow}
          </p>
          <h2 className="font-display text-4xl md:text-5xl tracking-tightest leading-[1.05] text-balance">
            {mobile.title}
          </h2>
          <p className="mt-6 text-lg text-ink-200 leading-relaxed max-w-lg">
            {mobile.body}
          </p>
          <div className="mt-8 flex items-center gap-6">
            <ButtonLink href={mobile.cta.href} variant="secondary" size="lg">
              {mobile.cta.label}
            </ButtonLink>
            <div className="flex items-center gap-3">
              <QRCode />
              <div className="text-xs text-ink-300 leading-tight max-w-[120px]">
                Scan with your iPhone camera to install.
              </div>
            </div>
          </div>
        </div>
        <div>
          <PhoneFrame />
        </div>
      </div>
    </Section>
  );
}
