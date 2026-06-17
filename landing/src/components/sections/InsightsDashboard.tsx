"use client";

import { useState } from "react";
import { motion, useReducedMotion } from "framer-motion";
import { Info } from "lucide-react";
import {
  Section,
  SectionEyebrow,
  SectionTitle,
  SectionLede,
} from "@/components/ui/Section";
import { insights } from "@/lib/content";
import { cn } from "@/lib/cn";

/* ========================================================================== */
/*  InsightsDashboard — interactive CSS-mock recreation of the Airnote app's  */
/*  "Insights" view. Click the time-range tabs at the top to swap every      */
/*  metric (WPM, sessions, words, accuracy, streak heatmap) to that range's  */
/*  snapshot. Hover heatmap cells for date + word-count tooltips. Hover any  */
/*  tile for subtle lift + glow.                                            */
/* ========================================================================== */

type RangeId = (typeof insights.ranges)[number]["id"];
type Range = (typeof insights.ranges)[number];

const ACCENT = "#a5b4fc"; // periwinkle (matches --accent)

/* ───────────────────────── Date helpers for heatmap tooltip ──────────────── */

const HEATMAP_COLS = 12;
const END_DATE = new Date(insights.streakBase.endDate);

/** Given a (day, col) cell offset, return the calendar date. Cell at
 *  (day 6 / Sat, col 11) is the configured endDate; earlier cells walk
 *  backward 1 day at a time. */
function dateForCell(day: number, col: number): Date {
  const daysFromEnd = (HEATMAP_COLS - 1 - col) * 7 + (6 - day);
  const d = new Date(END_DATE);
  d.setDate(d.getDate() - daysFromEnd);
  return d;
}

const DATE_FMT = new Intl.DateTimeFormat("en-US", {
  month: "short",
  day: "numeric",
  year: "numeric",
});

/* ───────────────────────────── Arc gauge (WPM tile) ─────────────────────── */

function ArcGauge({
  value,
  label,
  valueLabel,
  animate,
}: {
  value: number;
  label: string;
  valueLabel: string;
  animate: boolean;
}) {
  const ARC = Math.PI * 40;
  const filled = (Math.max(0, Math.min(100, value)) / 100) * ARC;
  return (
    <div className="relative w-full max-w-[160px] mx-auto">
      <svg viewBox="0 0 100 60" className="w-full">
        <path
          d="M 10 50 A 40 40 0 0 1 90 50"
          stroke="rgba(255,255,255,0.08)"
          strokeWidth="6"
          strokeLinecap="round"
          fill="none"
        />
        <motion.path
          d="M 10 50 A 40 40 0 0 1 90 50"
          stroke={ACCENT}
          strokeWidth="6"
          strokeLinecap="round"
          fill="none"
          strokeDasharray={`${ARC} ${ARC}`}
          initial={animate ? { strokeDashoffset: ARC } : false}
          animate={{ strokeDashoffset: ARC - filled }}
          transition={{ duration: 1.0, ease: [0.22, 1, 0.36, 1], delay: 0.1 }}
        />
      </svg>
      <div className="absolute inset-x-0 bottom-0 text-center">
        <div className="text-[10px] uppercase tracking-[0.15em] text-ink-300">
          {label}
        </div>
        <div className="text-sm font-semibold text-ink-50">{valueLabel}</div>
      </div>
    </div>
  );
}

/* ─────────────────────────── Accuracy ring tile ─────────────────────────── */

function AccuracyRing({
  percent,
  centerLabel,
  animate,
}: {
  percent: number;
  centerLabel: string;
  animate: boolean;
}) {
  const R = 60;
  const C = 2 * Math.PI * R;
  const filled = (Math.max(0, Math.min(100, percent)) / 100) * C;
  return (
    <div className="relative w-[150px] h-[150px] mx-auto">
      <svg viewBox="0 0 160 160" className="w-full h-full -rotate-90">
        <circle
          cx="80"
          cy="80"
          r={R}
          stroke="rgba(255,255,255,0.08)"
          strokeWidth="10"
          fill="none"
        />
        <motion.circle
          cx="80"
          cy="80"
          r={R}
          stroke={ACCENT}
          strokeWidth="10"
          strokeLinecap="round"
          fill="none"
          strokeDasharray={`${C} ${C}`}
          initial={animate ? { strokeDashoffset: C } : false}
          animate={{ strokeDashoffset: C - filled }}
          transition={{ duration: 1.1, ease: [0.22, 1, 0.36, 1], delay: 0.15 }}
        />
      </svg>
      <div className="absolute inset-0 flex flex-col items-center justify-center">
        <div className="text-2xl font-bold text-ink-50">{percent}%</div>
        <div className="text-[11px] text-ink-300 mt-0.5">{centerLabel}</div>
      </div>
    </div>
  );
}

/* ─────────────────────────── Streak heatmap tile ────────────────────────── */
/* Hoverable / focusable cells with native title-attribute tooltips
   showing the date + word count (or "No dictation" for empty cells). */

type FilledCell = {
  day: number;
  col: number;
  today?: boolean;
  ghost?: boolean;
  words?: number;
};

function StreakHeatmap({
  daysOfWeek,
  months,
  filled,
  legend,
  emptyLabel,
  animate,
}: {
  daysOfWeek: readonly string[];
  months: readonly string[];
  filled: readonly FilledCell[];
  legend: { more: string; less: string; today: string };
  emptyLabel: string;
  animate: boolean;
}) {
  const filledMap = new Map<string, FilledCell>();
  filled.forEach((f) => filledMap.set(`${f.day}:${f.col}`, f));

  return (
    <div className="w-full">
      <div className="ml-9 grid grid-cols-3 mb-1 text-[10px] text-ink-300">
        {months.map((m) => (
          <span key={m}>{m}</span>
        ))}
      </div>

      <div className="flex gap-2">
        <div className="flex flex-col gap-1 text-[10px] text-ink-300 w-7 leading-[14px]">
          {daysOfWeek.map((d) => (
            <span key={d}>{d}</span>
          ))}
        </div>

        <div className="flex-1 grid grid-rows-7 grid-flow-col gap-1 auto-cols-fr">
          {Array.from({ length: 7 * HEATMAP_COLS }).map((_, idx) => {
            const day = idx % 7;
            const col = Math.floor(idx / 7);
            const key = `${day}:${col}`;
            const f = filledMap.get(key);
            const isFilled = !!f && !f.ghost;
            const isToday = !!f?.today;
            const isGhost = !!f?.ghost;
            const date = dateForCell(day, col);
            const dateLabel = DATE_FMT.format(date);
            const tooltip = isFilled
              ? `${dateLabel} — ${f?.words ?? 0} polished words${
                  isToday ? " · Today" : ""
                }`
              : isGhost
                ? `${dateLabel} — partial`
                : `${dateLabel} — ${emptyLabel}`;

            return (
              <motion.button
                key={key}
                type="button"
                aria-label={tooltip}
                title={tooltip}
                initial={animate ? { opacity: 0, scale: 0.6 } : false}
                animate={{ opacity: 1, scale: 1 }}
                transition={{
                  duration: 0.4,
                  delay: animate ? Math.min(0.5, col * 0.025) : 0,
                  ease: [0.22, 1, 0.36, 1],
                }}
                className={cn(
                  "block h-3.5 w-3.5 rounded-[3px] transition-transform duration-150",
                  "hover:scale-[1.5] focus:scale-[1.5] focus:outline-none",
                )}
                style={{
                  background: isFilled
                    ? "var(--accent-success)"
                    : isGhost
                      ? "rgba(74, 222, 128, 0.20)"
                      : "rgba(255,255,255,0.06)",
                  outline: isToday ? `1.5px solid ${ACCENT}` : "none",
                  outlineOffset: isToday ? "1px" : "0",
                }}
              />
            );
          })}
        </div>
      </div>

      <div className="mt-4 flex items-center justify-between text-[11px] text-ink-300">
        <div className="flex items-center gap-1.5">
          <span>{legend.more}</span>
          {[0.32, 0.5, 0.7, 1].map((alpha, i) => (
            <span
              key={i}
              aria-hidden
              className="block h-2.5 w-2.5 rounded-[2px]"
              style={{ background: `rgba(74, 222, 128, ${alpha})` }}
            />
          ))}
          <span className="ml-1">{legend.less}</span>
        </div>
        <div className="flex items-center gap-1.5">
          <span
            aria-hidden
            className="block h-2.5 w-2.5 rounded-[2px]"
            style={{ outline: `1.5px solid ${ACCENT}`, outlineOffset: "1px" }}
          />
          <span>{legend.today}</span>
        </div>
      </div>
    </div>
  );
}

/* ─────────────────────────────── Tile wrappers ───────────────────────────── */
/* Both KPITile and BigTile share a subtle hover lift + periwinkle halo +
   cursor-pointer so the whole dashboard reads as interactive. */

const TILE_HOVER =
  "transition-all duration-200 cursor-pointer hover:bg-ink-800/40 " +
  "hover:scale-[1.005] hover:shadow-[0_8px_24px_-12px_rgba(165,180,252,0.22)]";

function KPITile({
  children,
  className = "",
}: {
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <div
      className={cn(
        "rounded-2xl bg-ink-900/70 hairline p-5 md:p-6 flex flex-col",
        TILE_HOVER,
        className,
      )}
    >
      {children}
    </div>
  );
}

function BigTile({
  children,
  className = "",
}: {
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <div
      className={cn(
        "rounded-2xl bg-ink-900/70 hairline p-5 md:p-6",
        TILE_HOVER,
        className,
      )}
    >
      {children}
    </div>
  );
}

/* ─────────────────────────────── Range tabs ────────────────────────────── */

function RangeTabs({
  ranges,
  active,
  onChange,
  ariaLabel,
}: {
  ranges: readonly { id: RangeId; label: string }[];
  active: RangeId;
  onChange: (id: RangeId) => void;
  ariaLabel: string;
}) {
  return (
    <div
      role="tablist"
      aria-label={ariaLabel}
      className="flex flex-wrap items-center gap-1"
    >
      {ranges.map((r) => {
        const isActive = r.id === active;
        return (
          <button
            key={r.id}
            type="button"
            role="tab"
            aria-selected={isActive}
            onClick={() => onChange(r.id)}
            className={cn(
              "rounded-full px-3 py-1 text-sm transition-colors duration-200",
              "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#a5b4fc]/40",
              isActive
                ? "font-medium"
                : "text-ink-300 hover:text-ink-50 hover:bg-ink-50/5",
            )}
            style={
              isActive
                ? {
                    background: "rgba(165, 180, 252, 0.16)",
                    color: ACCENT,
                  }
                : undefined
            }
          >
            {r.label}
          </button>
        );
      })}
    </div>
  );
}

/* ─────────────────────────── Metric subtree ─────────────────────────────── */
/* Renders the per-range data block. Wrapped in a key={range.id} parent so a
   range change unmounts + remounts this entire subtree — the existing
   entrance animations (arc sweep, ring draw, heatmap stagger) replay
   naturally without any custom animation state. */

function MetricSubtree({
  range,
  animate,
}: {
  range: Range;
  animate: boolean;
}) {
  const { kpis, accuracy, streak } = range;
  const { wpm, sessions, total } = kpis;
  return (
    <>
      {/* KPI tiles row */}
      <div className="mt-6 grid grid-cols-1 md:grid-cols-3 gap-4">
        {/* WPM */}
        <KPITile>
          <div className="text-5xl md:text-6xl font-bold text-ink-50 tracking-tight leading-none">
            {wpm.value}
          </div>
          <div className="mt-2 text-[11px] uppercase tracking-[0.15em] text-ink-300 font-medium">
            Words per minute
          </div>
          <div className="mt-6">
            <ArcGauge
              value={wpm.gauge}
              label={wpm.gaugeLabel}
              valueLabel={wpm.gaugeValue}
              animate={animate}
            />
          </div>
        </KPITile>

        {/* Sessions */}
        <KPITile>
          <div className="text-5xl md:text-6xl font-bold text-ink-50 tracking-tight leading-none">
            {sessions.value}
          </div>
          <div className="mt-2 text-[11px] uppercase tracking-[0.15em] text-ink-300 font-medium">
            {sessions.label}
          </div>
          <ul className="mt-6 space-y-2.5">
            {sessions.lines.map((line, i) => (
              <li
                key={i}
                className="flex items-center justify-between text-sm text-ink-100"
              >
                <span>{line.text}</span>
                <Info
                  className="h-3.5 w-3.5 text-ink-300"
                  strokeWidth={2}
                  aria-hidden
                />
              </li>
            ))}
          </ul>
        </KPITile>

        {/* Total */}
        <KPITile className="relative overflow-hidden">
          <div
            aria-hidden
            className="absolute inset-0 pointer-events-none"
            style={{
              background:
                "radial-gradient(ellipse 60% 60% at 100% 100%, rgba(165,180,252,0.10), transparent 60%)",
            }}
          />
          <div className="relative">
            <div className="text-5xl md:text-6xl font-bold text-ink-50 tracking-tight leading-none">
              {total.value}
            </div>
            <div className="mt-2 text-[11px] uppercase tracking-[0.15em] text-ink-300 font-medium">
              {total.label}
            </div>
            {total.sub && (
              <div className="mt-5 text-sm font-semibold text-ink-50">
                {total.sub}
              </div>
            )}
            <ul className="mt-2 space-y-1.5">
              {total.lines.map((line, i) => (
                <li key={i} className="text-sm text-ink-200 leading-snug">
                  {line.text}
                </li>
              ))}
            </ul>
          </div>
        </KPITile>
      </div>

      {/* Detail tiles row */}
      <div className="mt-4 grid grid-cols-1 md:grid-cols-2 gap-4">
        {/* Accuracy */}
        <BigTile>
          <div className="flex items-baseline justify-between">
            <h4 className="text-lg font-semibold text-ink-50">
              {accuracy.title}
            </h4>
            <span className="text-[11px] uppercase tracking-[0.15em] text-ink-300">
              {accuracy.sessions}
            </span>
          </div>
          <div className="mt-5 mb-4">
            <AccuracyRing
              percent={accuracy.percent}
              centerLabel={accuracy.centerLabel}
              animate={animate}
            />
          </div>
          <ul className="space-y-2.5 border-t border-ink-50/5 pt-4">
            {accuracy.rows.map((r) => (
              <li
                key={r.label}
                className="flex items-center justify-between text-sm"
              >
                <span className="flex items-center gap-2 text-ink-200">
                  <span
                    aria-hidden
                    className="inline-block h-3 w-3 rounded-full border border-ink-50/15"
                  />
                  {r.label}
                </span>
                <span className="font-semibold text-ink-50">{r.value}</span>
              </li>
            ))}
          </ul>
          <div className="mt-4 pt-4 border-t border-ink-50/5 flex items-center justify-between text-sm">
            <span className="text-ink-200">{accuracy.last.label}</span>
            <span className="font-semibold text-ink-50">
              {accuracy.last.value}
            </span>
          </div>
        </BigTile>

        {/* Streak */}
        <BigTile>
          <div className="flex items-baseline justify-between mb-5">
            <h4 className="text-lg font-semibold text-ink-50">
              {streak.title}
            </h4>
            <span className="text-[11px] uppercase tracking-[0.15em] text-ink-300">
              {streak.best}
            </span>
          </div>
          <StreakHeatmap
            daysOfWeek={insights.streakBase.daysOfWeek}
            months={insights.streakBase.months}
            filled={streak.filled}
            legend={insights.streakBase.legend}
            emptyLabel={insights.streakBase.emptyLabel}
            animate={animate}
          />
        </BigTile>
      </div>
    </>
  );
}

/* ──────────────────────────────── Section ────────────────────────────────── */

export function InsightsDashboard() {
  const reduce = useReducedMotion();
  const animate = !reduce;

  const [activeRange, setActiveRange] = useState<RangeId>(insights.defaultRange);
  const currentRange =
    insights.ranges.find((r) => r.id === activeRange) ?? insights.ranges[0];

  const rangeTabsMeta = insights.ranges.map((r) => ({
    id: r.id,
    label: r.label,
  }));

  return (
    <Section id="insights">
      <div className="max-w-3xl">
        <SectionEyebrow>{insights.eyebrow}</SectionEyebrow>
        <SectionTitle>{insights.title}</SectionTitle>
        <SectionLede>{insights.subtitle}</SectionLede>
      </div>

      {/* Dashboard mock card */}
      <motion.div
        initial={reduce ? false : { opacity: 0, y: 24 }}
        whileInView={{ opacity: 1, y: 0 }}
        viewport={{ once: true, margin: "-10% 0px" }}
        transition={{ duration: 0.7, ease: [0.22, 1, 0.36, 1] }}
        className="mt-14 md:mt-16 rounded-3xl bg-ink-800 hairline overflow-hidden"
        style={{
          background:
            "linear-gradient(180deg, rgba(22,22,28,0.95) 0%, rgba(13,13,18,0.95) 100%)",
          boxShadow:
            "0 30px 60px -20px rgba(0,0,0,0.5), inset 0 1px 0 rgba(255,255,255,0.04)",
        }}
      >
        <div className="p-6 md:p-10">
          {/* Inner header — title left, range tabs right */}
          <div className="flex flex-col md:flex-row md:items-start md:justify-between gap-4">
            <div>
              <h3 className="font-display text-3xl md:text-4xl tracking-tight text-ink-50">
                {insights.innerHeader.title}
              </h3>
              <div className="mt-2 flex items-center gap-2">
                <span
                  aria-hidden
                  className="h-1.5 w-1.5 rounded-full"
                  style={{ background: ACCENT }}
                />
                <span className="text-sm text-ink-200">
                  Your recording analytics · {currentRange.subhead}
                </span>
              </div>
            </div>

            <RangeTabs
              ranges={rangeTabsMeta}
              active={activeRange}
              onChange={setActiveRange}
              ariaLabel={insights.rangeLabel}
            />
          </div>

          {/* Metrics — key={range.id} forces remount so every entrance
              animation (arc sweep, ring draw, heatmap stagger) replays
              cleanly when the user clicks a new range. */}
          <div key={currentRange.id}>
            <MetricSubtree range={currentRange} animate={animate} />
          </div>
        </div>
      </motion.div>
    </Section>
  );
}
