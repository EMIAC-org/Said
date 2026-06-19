import type { Metadata } from "next";
import {
  ArrowDownToLine,
  BarChart2,
  Bell,
  BookOpen,
  Bug,
  CalendarDays,
  ChevronDown,
  Copy,
  Headphones,
  History,
  LayoutDashboard,
  Mic2,
  Radio,
  Settings,
  ShieldCheck,
  Sparkles,
  Sun,
  UserPlus,
  Video,
} from "lucide-react";
import { downloads } from "@/lib/content";

export const metadata: Metadata = {
  title: "Airnote Changelog",
  description: "Latest Airnote releases, stability fixes, and download links.",
};

const latest = {
  version: "2.4.0",
  date: "Jun 18, 2026",
  title: "Learning, review card, and light-mode polish",
  intro:
    "AirNote 2.4.0 fixes the recent learning-review regression, keeps actionable review cards visible, routes server polishing through the safer smart path, and refreshes the desktop colors for light mode.",
  sections: [
    {
      id: "learning-pipeline",
      eyebrow: "#Learning",
      title: "Single correction learning is restored",
      icon: ShieldCheck,
      body: [
        "Server-side review candidates no longer block the existing safe auto-learn path when the edit is one clear STT correction.",
        "Corrections such as product names, tools, and short jargon terms can again pass through the local single-change validation and alias-safety gates.",
      ],
      bullets: [
        "Restores the safe single-change learning path",
        "Keeps ambiguous multi-candidate edits in review",
        "Preserves validation, alias-safety, and lexicon invalidation",
      ],
    },
    {
      id: "review-card",
      eyebrow: "#Review Card",
      title: "Review prompts stay visible",
      icon: Bell,
      body: [
        "The status-bar state machine now preserves interactive review and confirmation prompts through idle resyncs.",
        "Review cards are treated as actionable prompts, so they are not hidden by passive word-learned notification settings.",
      ],
      bullets: [
        "Prevents review cards from flashing and disappearing",
        "Stops paste auto-hide from overwriting action prompts",
        "Keeps passive notification preferences separate from review actions",
      ],
    },
    {
      id: "desktop-polish",
      eyebrow: "#Desktop Polish",
      title: "Light mode and runtime routing are cleaner",
      icon: Sun,
      body: [
        "The desktop UI uses clearer light-mode surfaces and token-based colors across the pages that looked washed out or mismatched.",
        "Server runtime requests route through smart-mode output so users on older fast-mode selections avoid degraded polishing while the server runtime is being unified.",
      ],
      bullets: [
        "Better light-mode contrast and hierarchy",
        "Meetings and Divo colors no longer depend on hardcoded dark fills",
        "Fast and smart selections use the safer smart server route",
      ],
    },
  ],
};

const noteGroups = [
  {
    title: "Learning",
    count: 3,
    items: [
      "Restored auto-learning for one clear STT correction after the server raw-judge path returns one candidate.",
      "Kept multi-candidate and ambiguous learning edits in review instead of auto-persisting them.",
      "Preserved centralized validation, alias-safety checks, lexicon invalidation, and retrain scheduling.",
    ],
  },
  {
    title: "Review UI",
    count: 3,
    items: [
      "Preserved review, confirmation, error, and paste/manual-paste HUD states through idle status-bar resyncs.",
      "Separated actionable review cards from the passive Word learned notification toggle.",
      "Stopped paste auto-hide timers from clearing review and confirmation prompts.",
    ],
  },
  {
    title: "Verification",
    count: 3,
    items: [
      "Validated the full repository gate with just check before merging to main.",
      "Built, signed, notarized, stapled, and Gatekeeper-verified the Apple Silicon DMG.",
      "Published the Darwin updater manifest while leaving the Windows updater manifest untouched.",
    ],
  },
];

const releaseDownloads = [
  {
    version: "2.4.0",
    date: "Jun 18, 2026",
    title: "Latest stable",
    downloads: [{ platform: "Mac", label: "Mac DMG", href: downloads.mac.latestDmg }],
  },
  {
    version: "2.3.9",
    date: "Jun 17, 2026",
    title: "Crash-hardened desktop runtime",
    downloads: [],
  },
  {
    version: "2.3.8",
    date: "Jun 17, 2026",
    title: "Cleaner stream shutdown",
    downloads: [],
  },
  {
    version: "2.3.7",
    date: "Jun 15, 2026",
    title: "Bluetooth-safe dictation",
    downloads: [],
  },
  {
    version: "2.3.6",
    date: "Jun 2026",
    title: "Stable desktop release",
    downloads: [],
  },
];

function AppleMark({ className = "h-4 w-4" }: { className?: string }) {
  // Classic Apple Inc. brand silhouette (bitten apple) — not the lucide fruit icon.
  return (
    <svg viewBox="0 0 384 512" fill="currentColor" aria-hidden className={className}>
      <path d="M318.7 268.7c-.2-36.7 16.4-64.4 50-84.8-18.8-26.9-47.2-41.7-84.7-44.6-35.5-2.8-74.3 20.7-88.5 20.7-15 0-49.4-19.7-76.4-19.7C63.3 141.2 4 184.8 4 273.5q0 39.3 14.4 81.2c12.8 36.7 59 126.7 107.2 125.2 25.2-.6 43-17.9 75.8-17.9 31.8 0 48.3 17.9 76.4 17.9 48.6-.7 90.4-82.5 102.6-119.3-65.2-30.7-61.7-90-61.7-91.9zm-56.6-164.2c27.3-32.4 24.8-61.9 24-72.5-24.1 1.4-52 16.4-67.9 34.9-17.5 19.8-27.8 44.3-25.6 71.9 26.1 2 49.9-11.4 69.5-34.3z" />
    </svg>
  );
}

function AirnoteMark() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden className="h-6 w-6 text-white">
      <rect x="3" y="8.5" width="3" height="7" rx="1.5" fill="currentColor" />
      <rect x="8" y="4.5" width="3" height="15" rx="1.5" fill="currentColor" />
      <rect x="13" y="2.5" width="3" height="19" rx="1.5" fill="currentColor" />
      <rect x="18" y="6.5" width="3" height="11" rx="1.5" fill="currentColor" />
    </svg>
  );
}

function DownloadIcon({ platform }: { platform: string }) {
  if (platform === "Mac") {
    return <AppleMark className="h-4 w-4" />;
  }
  if (platform === "Windows") {
    return (
      <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden className="h-4 w-4">
        <rect x="3" y="3" width="8.5" height="8.5" rx="0.5" />
        <rect x="12.5" y="3" width="8.5" height="8.5" rx="0.5" />
        <rect x="3" y="12.5" width="8.5" height="8.5" rx="0.5" />
        <rect x="12.5" y="12.5" width="8.5" height="8.5" rx="0.5" />
      </svg>
    );
  }
  return <ArrowDownToLine className="h-4 w-4" />;
}

// ── Faithful AirNote desktop reproduction — an "HTML video" ────────────────────
//
// Pixel-for-pixel rebuild of the real AirNote dashboard (EditorialDashboard +
// Sidebar + Topbar), using the exact design tokens from desktop/src/styles.css.
// It loops through the app's REAL in-use states — READY → RECORDING → live
// polish typing → landed in Today — so every element shown is a genuine AirNote
// UI element, not an invented screen. Pure CSS keyframes (no JS) so it works in
// the static Next.js export.

// Design tokens, mirrored from desktop/src/styles.css (dark theme).
const T = {
  bg:        "hsl(240 6% 6%)",
  fg:        "hsl(240 6% 92%)",
  muted:     "hsl(240 5% 60%)",
  primary:   "hsl(226 80% 78%)",
  border:    "hsl(240 5% 16%)",
  surface3:  "hsl(240 5% 7.5%)",
  surface4:  "hsl(240 5% 13%)",
  recording: "hsl(354 84% 62%)",
  glassBg:   "hsl(240 5% 9% / 0.55)",
  stroke:    "hsl(0 0% 100% / 0.06)",
  pillBg:    "hsl(0 0% 98%)",
  pillFg:    "hsl(240 8% 8%)",
  chipBg:    "hsl(226 80% 78% / 0.14)",
  chipFg:    "hsl(226 80% 84%)",
} as const;

const APP_FONT =
  '-apple-system, BlinkMacSystemFont, "SF Pro Text", "SF Pro Display", system-ui, sans-serif';
const MONO_FONT = 'ui-monospace, "SF Mono", "JetBrains Mono", Menlo, monospace';

// Believable 14-day activity, height in px (max 64) — alternating accent bars.
const ACTIVITY = [10, 30, 14, 42, 8, 26, 18, 52, 12, 38, 22, 60, 16, 46];

const APP_KEYFRAMES = `
  @keyframes anReady { 0%,16%{opacity:1} 20%{opacity:0} 84%{opacity:0} 90%,100%{opacity:1} }
  @keyframes anRec   { 0%,18%{opacity:0} 22%{opacity:1} 82%{opacity:1} 86%,100%{opacity:0} }
  @keyframes anStrip {
    0%,20%   { opacity:0; transform:translateY(6px); }
    26%,72%  { opacity:1; transform:translateY(0); }
    80%,100% { opacity:0; transform:translateY(6px); }
  }
  @keyframes anType  { 0%,24%{width:0} 58%,74%{width:var(--w)} 80%,100%{width:0} }
  @keyframes anEmpty { 0%,70%{opacity:1} 78%,94%{opacity:0} 99%,100%{opacity:1} }
  @keyframes anRow   { 0%,76%{opacity:0} 82%,93%{opacity:1} 98%,100%{opacity:0} }
  @keyframes anCaret { 0%,49%{opacity:1} 50%,100%{opacity:0} }
  @keyframes anRecPulse {
    0%,100% { box-shadow:0 0 0 0 hsl(354 84% 62% / 0.55); }
    70%     { box-shadow:0 0 0 5px hsl(354 84% 62% / 0); }
  }
  .av-ready { animation: anReady 11s ease-in-out infinite; }
  .av-rec   { animation: anRec   11s ease-in-out infinite; }
  .av-strip { animation: anStrip 11s ease-in-out infinite; }
  .av-type  { display:inline-block; white-space:nowrap; overflow:hidden; vertical-align:bottom;
              width:0; animation: anType 11s steps(30,end) infinite; }
  .av-empty { animation: anEmpty 11s ease-in-out infinite; }
  .av-row   { animation: anRow   11s ease-in-out infinite; }
  .av-caret { display:inline-block; width:2px; height:13px; margin-left:1px; vertical-align:-1px;
              background:hsl(226 80% 78%); animation: anCaret 1s steps(1) infinite; }
  .av-recdot { animation: anRecPulse 1.6s ease-in-out infinite; }
`;

const SECTION_LABEL: React.CSSProperties = {
  fontSize: 10,
  fontWeight: 700,
  letterSpacing: "0.14em",
  textTransform: "uppercase",
  color: T.muted,
};

function NavRow({
  icon,
  label,
  active = false,
  badge,
}: {
  icon: React.ReactNode;
  label: string;
  active?: boolean;
  badge?: string;
}) {
  return (
    <div
      className="flex items-center gap-2.5 rounded-lg px-3 py-1.5"
      style={{
        fontSize: 11.5,
        fontWeight: active ? 600 : 500,
        color: active ? T.pillFg : T.muted,
        background: active ? T.pillBg : "transparent",
      }}
    >
      <span className="flex-shrink-0" style={{ opacity: active ? 0.85 : 0.7 }}>
        {icon}
      </span>
      <span className="flex-1 truncate">{label}</span>
      {badge && (
        <span
          className="rounded px-1.5 py-0.5"
          style={{ fontSize: 9, fontWeight: 700, color: T.chipFg, background: T.chipBg }}
        >
          {badge}
        </span>
      )}
    </div>
  );
}

function DemoPanel({ title: _title, index }: { title: string; index: number }) {
  // Per-section dictated line — real Hinglish → polished Roman, exactly what
  // AirNote produces. Kept to one line so the typewriter reveal tracks cleanly.
  const phrases = [
    { polished: "Bluetooth pe music dictation ke baad bhi clear chalta hai.", words: 9, app: "Slack" },
    { polished: "AirNote ne headset mic avoid karke built-in mic chuna.",      words: 9, app: "Notion" },
    { polished: "Meeting recording ke dauraan playback clean rehti hai.",      words: 8, app: "Mail" },
  ];
  const p = phrases[index % phrases.length];

  return (
    <div
      className="relative my-8 w-full overflow-hidden rounded-[16px]"
      style={{
        aspectRatio: "16 / 10",
        background: T.bg,
        border: `1px solid ${T.stroke}`,
        boxShadow: "0 40px 120px -55px rgba(0,0,0,0.9), 0 2px 8px rgba(0,0,0,0.4)",
        fontFamily: APP_FONT,
      }}
    >
      {index === 0 && <style>{APP_KEYFRAMES}</style>}

      {/* Ambient wallpaper bleed — faint periwinkle wash at the edges (--mesh-bg). */}
      <div
        className="pointer-events-none absolute inset-0"
        style={{
          background:
            "radial-gradient(60% 50% at 8% 0%, hsl(226 80% 60% / 0.10) 0%, transparent 55%), radial-gradient(50% 50% at 100% 100%, hsl(220 60% 50% / 0.08) 0%, transparent 60%)",
        }}
      />

      <div className="absolute inset-0 grid" style={{ gridTemplateColumns: "210px 1fr" }}>
        {/* ── Sidebar — glass over the wallpaper ──────────────────────────── */}
        <aside
          className="relative flex flex-col"
          style={{
            background: T.glassBg,
            backdropFilter: "blur(40px) saturate(180%)",
            WebkitBackdropFilter: "blur(40px) saturate(180%)",
            borderRight: `1px solid ${T.stroke}`,
          }}
        >
          {/* Brand header — traffic lights + waveform mark */}
          <div className="flex h-12 flex-shrink-0 items-center gap-2.5 px-4">
            <span className="flex gap-1.5">
              <span className="h-2.5 w-2.5 rounded-full" style={{ background: "#ff5f57" }} />
              <span className="h-2.5 w-2.5 rounded-full" style={{ background: "#febc2e" }} />
              <span className="h-2.5 w-2.5 rounded-full" style={{ background: "#28c840" }} />
            </span>
            <span className="ml-2" style={{ color: T.primary }}>
              <AirnoteMark />
            </span>
          </div>

          {/* Nav */}
          <div className="flex flex-1 flex-col gap-5 overflow-hidden px-3 pt-3">
            <div>
              <p className="mb-1.5 px-3" style={SECTION_LABEL}>General</p>
              <div className="space-y-0.5">
                <NavRow icon={<LayoutDashboard size={15} />} label="Dashboard" active />
                <NavRow icon={<History size={15} />} label="History" />
                <NavRow icon={<BookOpen size={15} />} label="Vocabulary" />
                <NavRow icon={<BarChart2 size={15} />} label="Insights" badge="New" />
              </div>
            </div>
            <div>
              <p className="mb-1.5 px-3" style={SECTION_LABEL}>Enterprise</p>
              <div className="space-y-0.5">
                <NavRow icon={<Video size={15} />} label="Meetings" />
                <NavRow icon={<Sparkles size={15} />} label="Divo" />
              </div>
            </div>

            <div className="flex-1" />

            {/* Status card — READY ↔ RECORDING cross-fade (real app states) */}
            <div
              className="relative mb-1 rounded-xl p-3"
              style={{ background: T.glassBg, border: `1px solid ${T.stroke}`, minHeight: 52 }}
            >
              <div className="av-ready">
                <div className="mb-1.5 flex items-center gap-2">
                  <span
                    className="h-1.5 w-1.5 rounded-full"
                    style={{ background: T.primary, boxShadow: `0 0 8px ${T.primary}` }}
                  />
                  <span style={{ fontSize: 11, fontWeight: 600, letterSpacing: "0.04em", color: T.fg }}>
                    READY
                  </span>
                </div>
                <p style={{ fontSize: 11, color: T.muted }}>1,276 words · 4d streak</p>
              </div>
              <div className="av-rec absolute inset-0 p-3">
                <div className="mb-1.5 flex items-center gap-2">
                  <span className="av-recdot h-1.5 w-1.5 rounded-full" style={{ background: T.recording }} />
                  <span style={{ fontSize: 11, fontWeight: 600, letterSpacing: "0.04em", color: T.fg }}>
                    RECORDING
                  </span>
                </div>
                <p style={{ fontSize: 11, color: T.muted }}>Listening…</p>
              </div>
            </div>
          </div>

          {/* Footer nav */}
          <div className="flex-shrink-0 space-y-0.5 px-3 pb-3">
            <NavRow icon={<UserPlus size={15} />} label="Invite a friend" />
            <NavRow icon={<Settings size={15} />} label="Settings" />
            <NavRow icon={<Bug size={15} />} label="Report bug" />
          </div>
        </aside>

        {/* ── Main column — topbar + dashboard ────────────────────────────── */}
        <section className="relative flex min-w-0 flex-col">
          {/* Topbar — theme toggle, bell, avatar */}
          <div className="flex h-12 flex-shrink-0 items-center justify-end gap-2.5 pr-5">
            <span
              className="flex h-7 w-7 items-center justify-center rounded-full"
              style={{ color: T.muted }}
            >
              <Sun size={14} />
            </span>
            <span
              className="flex h-7 w-7 items-center justify-center rounded-full"
              style={{ color: T.muted }}
            >
              <Bell size={14} />
            </span>
            <span
              className="flex h-7 w-7 items-center justify-center rounded-full"
              style={{
                fontSize: 11,
                fontWeight: 700,
                background: "hsl(226 80% 78% / 0.18)",
                color: T.primary,
                boxShadow: "inset 0 0 0 1px hsl(226 80% 78% / 0.3)",
              }}
            >
              A
            </span>
          </div>

          {/* Dashboard body */}
          <div className="min-h-0 flex-1 overflow-hidden px-7 pb-2">
            {/* Live polish strip — only visible mid-loop while in flight */}
            <div
              className="av-strip relative mb-4 overflow-hidden rounded-xl px-4 py-3"
              style={{
                background:
                  "radial-gradient(80% 60% at 0% 0%, hsl(226 80% 78% / 0.12), transparent 60%), hsl(240 5% 7.5%)",
                boxShadow: "inset 0 0 0 1px hsl(0 0% 100% / 0.09)",
              }}
            >
              <div className="mb-1.5 flex items-center gap-2">
                <span
                  className="h-2 w-2 rounded-full"
                  style={{ background: T.primary, boxShadow: `0 0 10px ${T.primary}` }}
                />
                <span style={{ fontSize: 10, fontWeight: 700, letterSpacing: "0.14em", textTransform: "uppercase", color: T.primary }}>
                  Polishing with LLM
                </span>
              </div>
              <div style={{ fontSize: 13, lineHeight: 1.5, color: T.fg }}>
                <span className="av-type" style={{ ["--w" as string]: `${p.polished.length}ch` }}>
                  {p.polished}
                </span>
                <span className="av-caret" />
              </div>
            </div>

            {/* Hero */}
            <div className="mb-6">
              <p style={{ fontSize: 11, fontWeight: 600, letterSpacing: "0.16em", textTransform: "uppercase", color: T.primary, marginBottom: 8 }}>
                Today · Jun 17
              </p>
              <h3 style={{ margin: 0, fontSize: 26, fontWeight: 600, letterSpacing: "-0.025em", lineHeight: 1.15, color: T.fg }}>
                Ready when you are.
              </h3>
              <p style={{ marginTop: 8, fontSize: 13, lineHeight: 1.55, color: T.muted, maxWidth: 460 }}>
                Hold your hotkey and speak — AirNote types polished text into the focused app.
              </p>
            </div>

            {/* At a glance */}
            <div className="mb-6">
              <h4 style={{ margin: "0 0 10px", ...SECTION_LABEL, fontSize: 11 }}>At a glance</h4>
              <div
                className="grid"
                style={{
                  gridTemplateColumns: "repeat(3, 1fr)",
                  padding: "12px 0",
                  borderTop: `1px solid ${T.border}`,
                  borderBottom: `1px solid ${T.border}`,
                }}
              >
                {[
                  { label: "Avg pace", value: "132", unit: "wpm", border: false },
                  { label: "Polish latency", value: "556", unit: "ms", border: true },
                  { label: "Edits learned", value: "24", unit: "", border: true },
                ].map((g) => (
                  <div
                    key={g.label}
                    style={{ padding: "0 14px", borderLeft: g.border ? `1px solid ${T.border}` : "none" }}
                  >
                    <div style={{ fontSize: 10, letterSpacing: "0.12em", textTransform: "uppercase", color: T.muted }}>
                      {g.label}
                    </div>
                    <div style={{ marginTop: 5, fontSize: 22, fontWeight: 600, letterSpacing: "-0.02em", color: T.fg }}>
                      {g.value}
                      {g.unit && <span style={{ marginLeft: 5, fontSize: 12, fontWeight: 500, color: T.muted }}>{g.unit}</span>}
                    </div>
                  </div>
                ))}
              </div>
            </div>

            {/* Activity — last 14 days */}
            <div className="mb-6">
              <h4 style={{ margin: "0 0 10px", ...SECTION_LABEL, fontSize: 11 }}>Activity — last 14 days</h4>
              <div style={{ display: "flex", alignItems: "flex-end", gap: 4, height: 64 }}>
                {ACTIVITY.map((h, i) => (
                  <div
                    key={i}
                    style={{
                      flex: 1,
                      height: h,
                      borderRadius: 2,
                      background: i % 2 === 1 ? "hsl(226 80% 78% / 0.45)" : "hsl(0 0% 100% / 0.08)",
                    }}
                  />
                ))}
              </div>
              <div className="mt-2 flex justify-between" style={{ fontSize: 10.5, color: T.muted, fontFamily: MONO_FONT }}>
                <span>4</span>
                <span>11</span>
                <span>17</span>
              </div>
            </div>

            {/* Today — empty state ↔ landed recording cross-fade */}
            <div className="relative" style={{ minHeight: 48 }}>
              <div className="av-empty">
                <h4 style={{ margin: "0 0 8px", ...SECTION_LABEL, fontSize: 11 }}>Today — 0 recordings</h4>
                <p style={{ fontSize: 13, color: T.muted }}>Your first dictation will land here.</p>
              </div>
              <div className="av-row absolute inset-0">
                <h4 style={{ margin: "0 0 8px", ...SECTION_LABEL, fontSize: 11 }}>Today — 1 recording</h4>
                <div className="flex items-start gap-3 py-1.5" style={{ borderBottom: `1px solid ${T.border}` }}>
                  <div className="min-w-0 flex-1">
                    <div style={{ fontSize: 13, lineHeight: 1.35, color: T.fg }} className="truncate">
                      {p.polished}
                    </div>
                    <div className="mt-1 flex items-center gap-2" style={{ fontSize: 11, color: T.muted }}>
                      <span style={{ fontFamily: MONO_FONT }}>now</span>
                      <span style={{ fontSize: 10 }}>{p.words} words</span>
                      <span style={{ fontSize: 10 }}>{p.app}</span>
                    </div>
                  </div>
                  <Copy size={13} style={{ color: T.muted, marginTop: 2 }} />
                </div>
              </div>
            </div>
          </div>
        </section>
      </div>
    </div>
  );
}

export default function ChangelogPage() {
  return (
    <main className="min-h-screen bg-[var(--bg)] text-ink-50">
      <div className="pointer-events-none absolute inset-x-0 top-0 h-[1150px] overflow-hidden">
        <div className="absolute inset-x-0 top-0 h-[760px] hero-base-gradient opacity-35" />
        <div className="absolute inset-x-0 top-0 h-[680px] hero-highlight-top opacity-60" />
        <div className="absolute left-1/2 top-[220px] h-[640px] w-[1180px] -translate-x-1/2 rounded-full bg-accent/10 blur-[150px]" />
        <div className="absolute inset-x-0 top-[520px] h-[520px] bg-gradient-to-b from-transparent via-[rgba(13,13,18,0.72)] to-[var(--bg)]" />
      </div>

      <header className="container-page relative z-10 flex items-center justify-between py-7">
        <a href="/" className="flex items-center gap-2.5">
          <AirnoteMark />
          <span className="font-display text-lg tracking-tight">Airnote</span>
        </a>
        <nav className="flex items-center gap-2 text-sm text-white/70">
          <a href="/" className="rounded-full px-4 py-2 transition-colors hover:bg-white/8 hover:text-white">
            Product
          </a>
          <a href="/#pricing" className="rounded-full px-4 py-2 transition-colors hover:bg-white/8 hover:text-white">
            Pricing
          </a>
          <a href="/guide" className="rounded-full px-4 py-2 transition-colors hover:bg-white/8 hover:text-white">
            Guide
          </a>
          <a
            href={downloads.mac.latestDmg}
            className="rounded-full bg-white px-4 py-2 font-medium text-ink-900 transition-colors hover:bg-ink-50"
          >
            Download
          </a>
        </nav>
      </header>

      <div className="container-page relative z-10 grid gap-10 pb-24 pt-24 lg:grid-cols-[190px_minmax(0,760px)_1fr] lg:pt-32">
        <aside className="hidden pt-8 text-sm text-white/45 lg:block">
          <div className="sticky top-10 space-y-7">
            <div className="flex items-center gap-3">
              <span className="rounded-full border border-white/15 px-3 py-1 text-white/75">{latest.version}</span>
              <span>{latest.date}</span>
            </div>
            <nav className="space-y-2">
              {latest.sections.map((section) => (
                <a key={section.id} href={`#${section.id}`} className="block transition-colors hover:text-white">
                  {section.eyebrow}
                </a>
              ))}
              <a href="#downloads" className="block transition-colors hover:text-white">
                #Downloads
              </a>
              <a href="/guide" className="mt-5 flex items-center gap-2 rounded-full border border-white/10 bg-white/[0.04] px-3 py-2 text-white/70 transition-colors hover:bg-white/8 hover:text-white">
                <BookOpen className="h-4 w-4" />
                User guide
              </a>
            </nav>
          </div>
        </aside>

        <article className="min-w-0">
          <header className="mb-16">
            <div className="mb-3 text-lg font-medium text-white/45">Changelog</div>
            <h1 className="font-display text-5xl leading-[1.03] tracking-tightest text-white md:text-6xl">
              {latest.title}
            </h1>
            <p className="mt-8 max-w-[690px] text-lg leading-8 text-white/72">{latest.intro}</p>
            <div className="mt-8 flex flex-wrap gap-3">
              <a
                href={downloads.mac.latestDmg}
                className="inline-flex h-11 items-center gap-2 rounded-full bg-white px-5 text-sm font-medium text-ink-900 transition-colors hover:bg-ink-50"
              >
                <AppleMark className="h-4 w-4" />
                Download {latest.version}
              </a>
              <a
                href="#downloads"
                className="inline-flex h-11 items-center rounded-full border border-white/10 bg-black/20 px-5 text-sm font-medium text-white/75 backdrop-blur-md transition-colors hover:bg-white/8 hover:text-white"
              >
                Previous versions
              </a>
              <a
                href="/guide"
                className="inline-flex h-11 items-center gap-2 rounded-full border border-white/10 bg-black/20 px-5 text-sm font-medium text-white/75 backdrop-blur-md transition-colors hover:bg-white/8 hover:text-white"
              >
                <BookOpen className="h-4 w-4" />
                User guide
              </a>
            </div>
          </header>

          {latest.sections.map((section, index) => {
            const Icon = section.icon;
            return (
              <section key={section.id} id={section.id} className="scroll-mt-10 border-t border-white/10 py-11 first:border-t-0 first:pt-0">
                <div className="mb-4 flex items-center gap-2 text-sm font-semibold text-accent">
                  <Icon className="h-4 w-4" />
                  {section.eyebrow}
                </div>
                <h2 className="max-w-[740px] text-3xl font-semibold leading-tight tracking-tight text-white">
                  {section.title}
                </h2>
                <div className="mt-5 max-w-[730px] space-y-4 text-[15px] leading-7 text-white/66">
                  {section.body.map((paragraph) => (
                    <p key={paragraph}>{paragraph}</p>
                  ))}
                </div>

                <DemoPanel title={section.title} index={index} />

                <ul className="grid gap-3 md:grid-cols-3">
                  {section.bullets.map((bullet) => (
                    <li key={bullet} className="rounded-2xl border border-white/10 bg-white/[0.035] p-4 text-xs leading-6 text-white/62">
                      <ShieldCheck className="mb-2 h-4 w-4 text-accent-success" />
                      {bullet}
                    </li>
                  ))}
                </ul>
              </section>
            );
          })}

          <section className="border-t border-white/10 py-11">
            <div className="mb-4 flex items-center gap-2 text-sm font-semibold text-accent">
              <CalendarDays className="h-4 w-4" />
              #Release Notes
            </div>
            <h2 className="text-3xl font-semibold tracking-tight text-white">Detailed changes</h2>
            <div className="mt-7 overflow-hidden rounded-2xl border border-white/10 bg-white/[0.035]">
              {noteGroups.map((group, index) => (
                <details key={group.title} className="group border-t border-white/10 p-5 first:border-t-0" open={index === 0}>
                  <summary className="flex cursor-pointer list-none items-center justify-between gap-4">
                    <span className="flex items-center gap-2.5">
                      <span className="text-sm font-semibold text-white">{group.title}</span>
                      <span className="rounded-full bg-white/8 px-2 py-0.5 text-[10px] font-semibold text-white/45">
                        {group.count}
                      </span>
                    </span>
                    <ChevronDown className="h-4 w-4 text-white/40 transition-transform group-open:rotate-180" />
                  </summary>
                  <ul className="mt-4 space-y-2.5">
                    {group.items.map((item) => (
                      <li key={item} className="flex gap-3 text-sm leading-6 text-white/65">
                        <span className="mt-2 h-1.5 w-1.5 shrink-0 rounded-full bg-accent" />
                        <span>{item}</span>
                      </li>
                    ))}
                  </ul>
                </details>
              ))}
            </div>
          </section>

          <section id="downloads" className="scroll-mt-10 border-t border-white/10 pt-11">
            <div className="mb-4 flex items-center gap-2 text-sm font-semibold text-accent">
              <Headphones className="h-4 w-4" />
              #Downloads
            </div>
            <h2 className="text-3xl font-semibold tracking-tight text-white">Stable downloads</h2>
            <div className="mt-7 divide-y divide-white/10 rounded-2xl border border-white/10 bg-white/[0.035]">
              {releaseDownloads.map((release) => (
                <div key={release.version} className="grid gap-4 p-5 md:grid-cols-[1fr_auto] md:items-center">
                  <div>
                    <div className="flex flex-wrap items-center gap-3">
                      <span className="rounded-full border border-white/12 px-3 py-1 text-sm font-medium text-white">
                        {release.version}
                      </span>
                      <span className="text-sm text-white/45">{release.date}</span>
                    </div>
                    <p className="mt-3 text-sm text-white/62">{release.title}</p>
                  </div>
                  <div className="flex flex-wrap gap-2">
                    {release.downloads.length > 0 ? (
                      release.downloads.map((download) => (
                        <a
                          key={download.href}
                          href={download.href}
                          className="inline-flex h-10 items-center gap-2 rounded-full border border-white/10 bg-black/20 px-4 text-sm font-medium text-white/78 transition-colors hover:bg-white/10 hover:text-white"
                        >
                          <DownloadIcon platform={download.platform} />
                          {download.label}
                        </a>
                      ))
                    ) : (
                      <span className="inline-flex h-10 items-center rounded-full border border-white/10 bg-black/20 px-4 text-sm font-medium text-white/45">
                        Archived artifact unavailable
                      </span>
                    )}
                  </div>
                </div>
              ))}
            </div>
          </section>
        </article>
      </div>
    </main>
  );
}
