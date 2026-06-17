"use client";

import { useState } from "react";
import * as Tabs from "@radix-ui/react-tabs";
import { AnimatePresence, motion, useReducedMotion } from "framer-motion";
import {
  Section,
  SectionEyebrow,
  SectionTitle,
  SectionLede,
} from "@/components/ui/Section";
import { appPreview } from "@/lib/content";
import { cn } from "@/lib/cn";
import {
  APP_PREVIEW_VIEWS,
  type AppPreviewViewKey,
} from "./AppPreviewMocks";

/* ─────────────────────────── macOS window chrome ─────────────────────────── */
/* Same three-traffic-light pattern as HeroDemo's MockWindow. Inline here
   so we don't pull in the demo-window dependency tree just for chrome. */

function WindowChrome({ title }: { title: string }) {
  return (
    <div className="flex items-center gap-1.5 px-4 py-3 border-b border-ink-50/5 bg-ink-800">
      <span className="h-3 w-3 rounded-full bg-red-400/80" />
      <span className="h-3 w-3 rounded-full bg-yellow-400/80" />
      <span className="h-3 w-3 rounded-full bg-green-400/80" />
      <span className="ml-3 text-xs text-ink-300 font-mono truncate">
        {title}
      </span>
    </div>
  );
}

/* ──────────────────────────────── Section ────────────────────────────────── */

export function AppPreview() {
  const reduce = useReducedMotion();
  const animate = !reduce;

  const [tabId, setTabId] = useState<string>(appPreview.tabs[0].id);
  const activeTab =
    appPreview.tabs.find((t) => t.id === tabId) ?? appPreview.tabs[0];

  return (
    <Section id="app-preview">
      <div className="grid lg:grid-cols-12 gap-12 lg:gap-16 items-center">
        {/* ─────────────── LEFT: copy + per-tab caption ─────────────── */}
        <div className="lg:col-span-5">
          <SectionEyebrow>{appPreview.eyebrow}</SectionEyebrow>
          <SectionTitle>{appPreview.title}</SectionTitle>
          <SectionLede>{appPreview.body}</SectionLede>

          {/* Per-tab caption — crossfades when the user clicks a new tab.
              Pre-allocated min-height so the layout doesn't jump on swap. */}
          <div className="mt-8 min-h-[6rem]">
            <AnimatePresence mode="wait">
              <motion.p
                key={activeTab.id}
                initial={animate ? { opacity: 0, y: 6 } : false}
                animate={{ opacity: 1, y: 0 }}
                exit={animate ? { opacity: 0, y: -6 } : undefined}
                transition={{ duration: 0.25, ease: [0.22, 1, 0.36, 1] }}
                className="text-base text-ink-200 leading-relaxed"
              >
                {activeTab.caption}
              </motion.p>
            </AnimatePresence>
          </div>
        </div>

        {/* ─────────────── RIGHT: tabbed window with screenshots ──── */}
        <div className="lg:col-span-7">
          <Tabs.Root value={tabId} onValueChange={setTabId}>
            <motion.div
              initial={reduce ? false : { opacity: 0, y: 24 }}
              whileInView={{ opacity: 1, y: 0 }}
              viewport={{ once: true, margin: "-10% 0px" }}
              transition={{ duration: 0.7, ease: [0.22, 1, 0.36, 1] }}
              className="rounded-2xl bg-ink-800 hairline overflow-hidden shadow-2xl shadow-black/40"
            >
              <WindowChrome title={`Airnote · ${activeTab.label}`} />

              {/* Tab triggers */}
              <Tabs.List
                aria-label="App settings"
                className="flex items-center gap-1 border-b border-ink-50/5 bg-ink-800 px-2"
              >
                {appPreview.tabs.map((t) => (
                  <Tabs.Trigger
                    key={t.id}
                    value={t.id}
                    className={cn(
                      "relative px-4 py-2.5 text-sm transition-colors",
                      "text-ink-200 hover:text-ink-50",
                      "data-[state=active]:text-ink-50",
                      "data-[state=active]:font-medium",
                      "focus-visible:outline-none",
                      "after:absolute after:left-3 after:right-3 after:-bottom-px",
                      "after:h-px after:bg-transparent after:transition-colors",
                      "data-[state=active]:after:bg-accent",
                    )}
                  >
                    {t.label}
                  </Tabs.Trigger>
                ))}
              </Tabs.List>

              {/* Mock UI canvas — renders one of the CSS-mock settings views
                  per active tab. Same crossfade animation as before; just
                  swapped from <img> to the appropriate <View> component. */}
              <div className="relative aspect-[16/10] bg-ink-900">
                <AnimatePresence mode="wait">
                  <motion.div
                    key={activeTab.id}
                    initial={animate ? { opacity: 0, scale: 1.01 } : false}
                    animate={{ opacity: 1, scale: 1 }}
                    exit={animate ? { opacity: 0, scale: 0.99 } : undefined}
                    transition={{ duration: 0.3, ease: [0.22, 1, 0.36, 1] }}
                    className="absolute inset-0"
                  >
                    {(() => {
                      const View =
                        APP_PREVIEW_VIEWS[
                          activeTab.id as AppPreviewViewKey
                        ];
                      return View ? <View /> : null;
                    })()}
                  </motion.div>
                </AnimatePresence>
              </div>
            </motion.div>
          </Tabs.Root>
        </div>
      </div>
    </Section>
  );
}
