import { Check } from "lucide-react";
import { useDashboardLayout, type DashboardLayout } from "@/lib/useDashboardLayout";
import { useTheme, type Theme, type ThemePreference } from "@/lib/useTheme";

/**
 * Appearance section in Settings. Two blocks:
 *   1. Theme      — Follow system / Dark / Warm Paper, each with a live mini preview.
 *   2. Dashboard  — Editorial vs Split home layout.
 * Theme is driven by the shared App-level source (passed in) so the picker and
 * the topbar Sun/Moon toggle never fall out of sync. Falls back to its own
 * {@link useTheme} instance if mounted without props.
 */
export function AppearanceSection({
  preference, onPreferenceChange,
}: {
  /** Accepted for API compatibility with the shared App source; the picker
      now drives everything through the preference (system/dark/light). */
  theme?: Theme;
  onThemeChange?: (t: Theme) => void;
  preference?: ThemePreference;
  onPreferenceChange?: (p: ThemePreference) => void;
} = {}) {
  const fallback = useTheme();
  const activePref  = preference ?? fallback.preference;
  const setPref     = onPreferenceChange ?? fallback.setPreference;
  const { layout, setLayout } = useDashboardLayout();

  return (
    <div className="space-y-8">
      {/* ── Theme ─────────────────────────────────────────────────────── */}
      <section className="space-y-5">
        <SectionHeader
          title="Theme"
          desc="Follow your system, or pin a look. Applies instantly across the app."
        />
        <div className="grid gap-3" style={{ gridTemplateColumns: "1fr 1fr 1fr" }}>
          <ThemeCard
            palette={LIGHT_PALETTE}
            splitPalette={DARK_PALETTE}
            title="System"
            desc="Match your OS light/dark automatically."
            selected={activePref === "system"}
            onSelect={() => setPref("system")}
          />
          <ThemeCard
            palette={DARK_PALETTE}
            title="Dark"
            desc="Near-black floor, periwinkle accent."
            selected={activePref === "dark"}
            onSelect={() => setPref("dark")}
          />
          <ThemeCard
            palette={LIGHT_PALETTE}
            title="Warm Paper"
            desc="Warm off-white, indigo accent."
            selected={activePref === "light"}
            onSelect={() => setPref("light")}
          />
        </div>
      </section>

      {/* ── Dashboard layout ──────────────────────────────────────────── */}
      <section className="space-y-5">
        <SectionHeader
          title="Dashboard layout"
          desc="Choose how the home view surfaces your activity. You can switch any time."
        />
        <div className="grid gap-3" style={{ gridTemplateColumns: "1fr 1fr" }}>
          <PreviewCard
            option="split"
            title="Insights ⟷ Timeline"
            desc="Two columns. Stats on the left, day-grouped recordings on the right. Highest density."
            selected={layout === "split"}
            onSelect={() => setLayout("split")}
          />
          <PreviewCard
            option="editorial"
            title="Editorial column"
            desc="Single column, magazine-style. Personalised headline and calmly sectioned blocks."
            selected={layout === "editorial"}
            onSelect={() => setLayout("editorial")}
          />
        </div>
      </section>
    </div>
  );
}

// ── Section header ───────────────────────────────────────────────────────────

function SectionHeader({ title, desc }: { title: string; desc: string }) {
  return (
    <header>
      <h2
        className="m-0"
        style={{ fontSize: 16, fontWeight: 600, color: "hsl(var(--foreground))", letterSpacing: "-0.01em" }}
      >
        {title}
      </h2>
      <p
        className="mt-1.5 mb-0"
        style={{ fontSize: 12.5, color: "hsl(var(--muted-foreground))", lineHeight: 1.55, maxWidth: 480 }}
      >
        {desc}
      </p>
    </header>
  );
}

// ── Theme card ───────────────────────────────────────────────────────────────

/** A theme's mini-preview palette — hardcoded so the card always renders in
    ITS theme's colors, regardless of the currently-active theme. */
interface Palette {
  floor: string; rail: string; card: string; line: string;
  text: string; sub: string; accent: string; accentInk: string;
}

const DARK_PALETTE: Palette = {
  floor:  "#0f0f13",
  rail:   "#0d0d11",
  card:   "#17171d",
  line:   "rgba(255,255,255,0.06)",
  text:   "rgba(233,233,238,0.90)",
  sub:    "rgba(233,233,238,0.20)",
  accent: "#c8b8ff",
  accentInk: "#15151b",
};

const LIGHT_PALETTE: Palette = {
  floor:  "hsl(40 30% 94.5%)",
  rail:   "hsl(36 26% 92%)",
  card:   "hsl(42 46% 99%)",
  line:   "hsl(36 20% 84%)",
  text:   "hsl(230 20% 24%)",
  sub:    "hsl(36 14% 76%)",
  accent: "hsl(232 74% 59%)",
  accentInk: "#ffffff",
};

function ThemeCard({
  palette, splitPalette, title, desc, selected, onSelect,
}: {
  palette: Palette;
  /** When set, the preview is split diagonally palette↘splitPalette — used by
      the "System" card to signal it follows both OS appearances. */
  splitPalette?: Palette;
  title: string;
  desc: string;
  selected: boolean;
  onSelect: () => void;
}) {
  return (
    <button
      onClick={onSelect}
      className="text-left transition-all"
      style={{
        padding: 14,
        borderRadius: 12,
        background: "hsl(var(--surface-3))",
        boxShadow: selected
          ? "inset 0 0 0 1px hsl(var(--primary) / 0.55), 0 0 0 3px hsl(var(--primary) / 0.10)"
          : "inset 0 0 0 1px hsl(var(--border))",
        cursor: "pointer",
      }}
      onMouseEnter={(e) => {
        if (!selected) e.currentTarget.style.boxShadow = "inset 0 0 0 1px hsl(var(--glass-stroke-strong))";
      }}
      onMouseLeave={(e) => {
        if (!selected) e.currentTarget.style.boxShadow = "inset 0 0 0 1px hsl(var(--border))";
      }}
    >
      {splitPalette ? (
        <div className="relative w-full rounded-lg overflow-hidden" style={{ aspectRatio: "16 / 10" }}>
          <ThemeMini p={palette} />
          {/* Dark half, clipped to a diagonal so the card reads "auto both". */}
          <div
            className="absolute inset-0"
            style={{ clipPath: "polygon(100% 0, 100% 100%, 0 100%)" }}
          >
            <ThemeMini p={splitPalette} />
          </div>
        </div>
      ) : (
        <ThemeMini p={palette} />
      )}
      <div className="flex items-center justify-between mt-3">
        <div className="text-[13px] font-semibold" style={{ color: "hsl(var(--foreground))" }}>
          {title}
        </div>
        <span
          className="w-5 h-5 rounded-full flex items-center justify-center transition-all"
          style={{
            background: selected ? "hsl(var(--primary))" : "transparent",
            boxShadow: selected ? "none" : "inset 0 0 0 1px hsl(var(--border))",
            color: selected ? "hsl(var(--primary-foreground))" : "transparent",
          }}
        >
          <Check size={12} strokeWidth={3} />
        </span>
      </div>
      <p className="m-0 mt-1" style={{ fontSize: 11.5, color: "hsl(var(--muted-foreground))", lineHeight: 1.5 }}>
        {desc}
      </p>
    </button>
  );
}

/** Miniature app rendering in a fixed palette — rail + content + accent CTA. */
function ThemeMini({ p }: { p: Palette }) {
  return (
    <div
      className="relative w-full rounded-lg overflow-hidden"
      style={{ aspectRatio: "16 / 10", background: p.floor, boxShadow: `inset 0 0 0 1px ${p.line}` }}
    >
      {/* Sidebar rail */}
      <div style={{ position: "absolute", left: 0, top: 0, bottom: 0, width: "24%", background: p.rail, borderRight: `1px solid ${p.line}` }}>
        <MiniBar x={16} y={9}  w={62} h={5} color={p.accent} r={2} />
        {[20, 30, 40].map((y, i) => (
          <MiniBar key={y} x={16} y={y} w={58} h={4} color={i === 0 ? p.text : p.sub} r={2} />
        ))}
        {/* status card */}
        <div style={{ position: "absolute", left: "10%", right: "10%", bottom: "9%", height: "22%", borderRadius: 4, background: p.card, boxShadow: `inset 0 0 0 1px ${p.line}` }} />
      </div>

      {/* Content mat */}
      <div style={{ position: "absolute", left: "28%", top: "8%", right: "6%", bottom: "8%", borderRadius: 6, background: p.card, boxShadow: `0 2px 8px rgba(20,22,40,0.06), inset 0 0 0 1px ${p.line}` }}>
        {/* kicker + headline */}
        <MiniBar x={7} y={11} w={22} h={5}  color={p.accent} r={2} />
        <MiniBar x={7} y={22} w={70} h={9}  color={p.text} r={2} />
        <MiniBar x={7} y={36} w={46} h={4}  color={p.sub} r={2} />
        {/* activity bars */}
        {[7, 14, 21, 28, 35, 42, 49, 56, 63].map((x, i) => {
          const h = [10, 16, 8, 20, 13, 24, 11, 18, 26][i];
          return <MiniBar key={x} x={x} y={78 - h} w={4} h={h} color={i % 2 ? p.accent : p.sub} r={1} />;
        })}
        {/* accent CTA pill */}
        <div style={{ position: "absolute", right: "7%", bottom: "10%", width: "26%", height: "13%", borderRadius: 999, background: p.accent }} />
      </div>
    </div>
  );
}

// ── Preview card ─────────────────────────────────────────────────────────────

function PreviewCard({
  option, title, desc, selected, onSelect,
}: {
  option: DashboardLayout;
  title: string;
  desc: string;
  selected: boolean;
  onSelect: () => void;
}) {
  return (
    <button
      onClick={onSelect}
      className="text-left transition-all"
      style={{
        padding: 14,
        borderRadius: 12,
        background: "hsl(var(--surface-3))",
        boxShadow: selected
          ? "inset 0 0 0 1px hsl(var(--primary) / 0.55), 0 0 0 3px hsl(var(--primary) / 0.10)"
          : "inset 0 0 0 1px hsl(var(--border))",
        cursor: "pointer",
      }}
      onMouseEnter={(e) => {
        if (!selected) e.currentTarget.style.boxShadow = "inset 0 0 0 1px hsl(var(--glass-stroke-strong))";
      }}
      onMouseLeave={(e) => {
        if (!selected) e.currentTarget.style.boxShadow = "inset 0 0 0 1px hsl(var(--border))";
      }}
    >
      {/* Miniature preview */}
      <div
        className="relative w-full rounded-lg overflow-hidden"
        style={{
          aspectRatio: "16 / 10",
          background: "hsl(var(--surface-2))",
          boxShadow: "inset 0 0 0 1px hsl(var(--border))",
        }}
      >
        {option === "split" ? <MiniSplit /> : <MiniEditorial />}
      </div>

      {/* Title + check */}
      <div className="flex items-center justify-between mt-3">
        <div
          className="text-[13px] font-semibold"
          style={{ color: "hsl(var(--foreground))" }}
        >
          {title}
        </div>
        <span
          className="w-5 h-5 rounded-full flex items-center justify-center transition-all"
          style={{
            background: selected ? "hsl(var(--primary))" : "transparent",
            boxShadow: selected ? "none" : "inset 0 0 0 1px hsl(var(--border))",
            color: selected ? "hsl(var(--primary-foreground))" : "transparent",
          }}
        >
          <Check size={12} strokeWidth={3} />
        </span>
      </div>
      <p
        className="m-0 mt-1"
        style={{ fontSize: 11.5, color: "hsl(var(--muted-foreground))", lineHeight: 1.5 }}
      >
        {desc}
      </p>
    </button>
  );
}

// ── Miniature renderings — pure SVG-like blocks ─────────────────────────────

function MiniBar({
  x, y, w, h, color = "hsl(var(--foreground) / 0.10)", r = 2,
}: { x: number; y: number; w: number; h: number; color?: string; r?: number }) {
  return (
    <div
      style={{
        position: "absolute",
        left: `${x}%`, top: `${y}%`,
        width: `${w}%`, height: `${h}%`,
        borderRadius: r,
        background: color,
      }}
    />
  );
}

function MiniSplit() {
  const accent = "hsl(var(--primary) / 0.55)";
  const fg     = "hsl(var(--foreground) / 0.85)";
  const muted  = "hsl(var(--foreground) / 0.20)";
  return (
    <>
      {/* Top mat padding suggestion */}
      <MiniBar x={4} y={4} w={92} h={92} color="hsl(var(--surface-1))" />

      {/* LEFT — pace card */}
      <MiniBar x={6} y={7}  w={42} h={28} color="hsl(var(--surface-3))" />
      <MiniBar x={9} y={11} w={14} h={3}  color={muted} />
      <MiniBar x={9} y={17} w={20} h={6}  color={fg} />
      {/* sparkline */}
      <MiniBar x={9}  y={28} w={3} h={3}  color={muted} />
      <MiniBar x={13} y={26} w={3} h={5}  color={accent} />
      <MiniBar x={17} y={27} w={3} h={4}  color={muted} />
      <MiniBar x={21} y={24} w={3} h={7}  color={accent} />
      <MiniBar x={25} y={25} w={3} h={6}  color={muted} />
      <MiniBar x={29} y={22} w={3} h={9}  color={accent} />
      <MiniBar x={33} y={26} w={3} h={5}  color={muted} />
      <MiniBar x={37} y={23} w={3} h={8}  color={accent} />
      <MiniBar x={41} y={21} w={3} h={10} color={accent} />

      {/* LEFT — 2 mini tiles */}
      <MiniBar x={6}  y={38} w={20} h={14} color="hsl(var(--surface-3))" />
      <MiniBar x={9}  y={41} w={10} h={3}  color={muted} />
      <MiniBar x={9}  y={46} w={12} h={5}  color={fg} />

      <MiniBar x={28} y={38} w={20} h={14} color="hsl(var(--surface-3))" />
      <MiniBar x={31} y={41} w={10} h={3}  color={muted} />
      <MiniBar x={31} y={46} w={12} h={5}  color={fg} />

      {/* LEFT — apps card */}
      <MiniBar x={6}  y={55} w={42} h={37} color="hsl(var(--surface-3))" />
      <MiniBar x={9}  y={58} w={14} h={3}  color={fg} />
      {[64, 70, 76, 82, 88].map((y, i) => (
        <span key={y}>
          <MiniBar x={9}  y={y} w={3} h={3} color={muted} />
          <MiniBar x={14} y={y} w={18} h={3} color={i === 0 ? fg : muted} />
          <MiniBar x={42} y={y} w={6}  h={3} color={muted} />
        </span>
      ))}

      {/* RIGHT — timeline */}
      <MiniBar x={50} y={7} w={46} h={85} color="hsl(var(--surface-3))" />
      <MiniBar x={53} y={10} w={14} h={3} color={fg} />
      {/* filter chips */}
      <MiniBar x={82} y={10} w={4} h={3}  color={accent} />
      <MiniBar x={87} y={10} w={3} h={3}  color={muted} />
      <MiniBar x={91} y={10} w={3} h={3}  color={muted} />
      {/* day label */}
      <MiniBar x={53} y={16} w={8} h={2}  color={muted} />
      {/* recording rows */}
      {[21, 28, 35, 42, 49, 56, 63, 70, 77, 84].map((y, i) => (
        <span key={y}>
          <MiniBar x={53} y={y}     w={5}  h={2} color={muted} />
          <MiniBar x={60} y={y}     w={28} h={2} color={i === 0 ? fg : muted} />
          <MiniBar x={60} y={y + 3} w={20} h={2} color={muted} />
          <MiniBar x={90} y={y}     w={4}  h={2} color={muted} />
        </span>
      ))}
    </>
  );
}

function MiniEditorial() {
  const accent = "hsl(var(--primary) / 0.55)";
  const fg     = "hsl(var(--foreground) / 0.85)";
  const muted  = "hsl(var(--foreground) / 0.18)";
  return (
    <>
      {/* mat */}
      <MiniBar x={4} y={4} w={92} h={92} color="hsl(var(--surface-1))" />

      {/* Hero — kicker + big headline */}
      <MiniBar x={20} y={10} w={10} h={2}  color={accent} />
      <MiniBar x={20} y={16} w={60} h={5}  color={fg} />
      <MiniBar x={20} y={24} w={40} h={2}  color={muted} />
      <MiniBar x={20} y={28} w={32} h={2}  color={muted} />

      {/* At a glance — 3-col row */}
      <MiniBar x={20} y={38} w={60} h={0.4} color="hsl(var(--foreground) / 0.10)" />
      <MiniBar x={22} y={41} w={8}  h={2} color={muted} />
      <MiniBar x={22} y={45} w={10} h={4} color={fg} />
      <MiniBar x={42} y={41} w={8}  h={2} color={muted} />
      <MiniBar x={42} y={45} w={10} h={4} color={fg} />
      <MiniBar x={62} y={41} w={8}  h={2} color={muted} />
      <MiniBar x={62} y={45} w={10} h={4} color={fg} />
      <MiniBar x={20} y={52} w={60} h={0.4} color="hsl(var(--foreground) / 0.10)" />

      {/* Activity bars */}
      <MiniBar x={20} y={58} w={14} h={2} color={muted} />
      {[20, 24.5, 29, 33.5, 38, 42.5, 47, 51.5, 56, 60.5, 65, 69.5, 74, 78.5].map((x, i) => {
        const h = [3.5, 5.5, 2.5, 6.5, 4.5, 7.5, 4, 5.5, 2, 8, 5, 6.5, 3, 6][i] ?? 4;
        return (
          <MiniBar
            key={x}
            x={x} y={75 - h} w={3} h={h}
            color={i % 2 === 1 ? accent : "hsl(var(--foreground) / 0.10)"}
          />
        );
      })}

      {/* Latest list */}
      <MiniBar x={20} y={82} w={8}  h={2} color={muted} />
      <MiniBar x={20} y={87} w={50} h={2} color={fg} />
      <MiniBar x={20} y={91} w={40} h={2} color={muted} />
    </>
  );
}
