import React, { useState, useRef, useEffect, useLayoutEffect } from "react";
import { createPortal } from "react-dom";
import {
  LayoutDashboard,
  History,
  BarChart2,
  BookOpen,
  Settings,
  HelpCircle,
  UserPlus,
  Mail,
  Copy,
  Check,
  Activity,
  Cpu,
  HardDrive,
  Server,
  Zap,
} from "lucide-react";
import { cn } from "@/lib/utils";
import { BrandMark } from "@/components/BrandMark";
import { getPerformanceSnapshot } from "@/lib/invoke";
import type { AppSnapshot, PerformanceSnapshot, ProcessPerf } from "@/types";

// ── Nav item type ──────────────────────────────────────────────────────────────

interface NavItem {
  id:       string;
  label:    string;
  icon:     React.ReactNode;
  badge?:   string;
  disabled?: boolean;
}

const GENERAL_NAV: NavItem[] = [
  { id: "dashboard",  label: "Dashboard",  icon: <LayoutDashboard size={15} /> },
  { id: "history",    label: "History",    icon: <History         size={15} /> },
  { id: "vocabulary", label: "Vocabulary", icon: <BookOpen        size={15} /> },
  { id: "insights",   label: "Insights",   icon: <BarChart2       size={15} />, badge: "New" },
];

// ── Nav button ─────────────────────────────────────────────────────────────────

function NavButton({
  item,
  isActive,
  onClick,
}: {
  item: NavItem;
  isActive: boolean;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      disabled={item.disabled}
      className={cn("nav-item", isActive && "active", item.disabled && "disabled")}
    >
      <span className="flex-shrink-0 opacity-70">{item.icon}</span>
      <span className="flex-1 truncate">{item.label}</span>
      {item.badge && (
        <span
          className="text-[9px] font-bold px-1.5 py-0.5 rounded flex-shrink-0"
          style={{
            color:      "hsl(var(--chip-cyan-fg))",
            background: "hsl(var(--chip-cyan-bg))",
          }}
        >
          {item.badge}
        </span>
      )}
    </button>
  );
}

// ── Props ──────────────────────────────────────────────────────────────────────

interface SidebarProps {
  snapshot:     AppSnapshot | null;
  activeView:   string;
  onViewChange: (view: string) => void;
  busy:         boolean;
  performanceMonitorEnabled?: boolean;
  onOpenInvite?: () => void;
}

// ── Component ──────────────────────────────────────────────────────────────────

export function Sidebar({
  snapshot,
  activeView,
  onViewChange,
  busy,
  performanceMonitorEnabled = false,
  onOpenInvite,
}: SidebarProps) {
  const isRecording  = snapshot?.state === "recording";
  const isProcessing = snapshot?.state === "processing" || busy;

  return (
    <aside
      className="flex flex-col h-full overflow-hidden flex-shrink-0 relative"
      style={{
        width:      "var(--sidebar-width)",
        background: "hsl(var(--surface-1))",
      }}
    >
      {/* ── Brand header — drag region + traffic light space ── */}
      <div className="flex items-center h-[var(--topbar-height)] px-4 flex-shrink-0">
        {/* macOS native traffic lights live in the top-left; reserve 70px so
            the brand mark doesn't sit under them. Windows puts its close/min/max
            buttons on the right, so we only need a 12px breathing gap there. */}
        <div
          data-tauri-drag-region
          className="flex-shrink-0 drag-region"
          style={{ width: snapshot?.platform === "windows" ? 12 : 70 }}
        />

        {/* Brand mark — single source of truth in BrandMark.tsx */}
        <div className="no-drag" title="Said — Voice Polish Studio">
          <BrandMark size={32} idSuffix="sidebar" />
        </div>
      </div>

      {/* ── Scrollable nav area ─────────────────────────────── */}
      <div className="flex-1 overflow-y-auto px-3 py-4 flex flex-col gap-6">

        {/* General section */}
        <section>
          <p className="section-label px-3 mb-2">General</p>
          <div className="space-y-0.5">
            {GENERAL_NAV.map((item) => (
              <NavButton
                key={item.id}
                item={item}
                isActive={activeView === item.id}
                onClick={() => !busy && onViewChange(item.id)}
              />
            ))}
          </div>
        </section>

        {/* Spacer */}
        <div className="flex-1" />

        {performanceMonitorEnabled && <PerformanceMonitor />}

        {/* Status card — glass */}
        <div className="rounded-xl glass p-3.5">
          <div className="flex items-center gap-2 mb-1.5">
            <div
              className={cn(
                "w-1.5 h-1.5 rounded-full flex-shrink-0",
                isRecording  ? "animate-pulse"   :
                isProcessing ? "animate-pulse" : ""
              )}
              style={{
                background: isRecording  ? "hsl(var(--recording))"
                          : isProcessing ? "hsl(38 90% 55%)"
                          :                "hsl(var(--primary))",
                boxShadow:  isRecording  ? "0 0 8px hsl(var(--recording) / 0.6)"
                          : isProcessing ? "0 0 8px hsl(38 90% 55% / 0.6)"
                          :                "0 0 8px hsl(var(--primary) / 0.5)",
              }}
            />
            <span className="text-[11px] font-semibold text-foreground tracking-wide">
              {isRecording  ? "RECORDING"  :
               isProcessing ? "PROCESSING" :
                              "READY"}
            </span>
          </div>
          <p className="text-[11px] text-muted-foreground leading-relaxed tabular-nums">
            {snapshot
              ? `${snapshot.total_words.toLocaleString()} words · ${snapshot.daily_streak}d streak`
              : "Loading…"}
          </p>
        </div>
      </div>

      {/* ── Footer nav — Invite / Settings / Help ─────────────────── */}
      <div className="px-3 py-3 flex-shrink-0 space-y-0.5">

        {/* Invite a friend — opens the in-app modal */}
        <button
          className="nav-item"
          onClick={() => onOpenInvite?.()}
        >
          <span className="flex-shrink-0 opacity-70">
            <UserPlus size={15} />
          </span>
          <span className="flex-1 truncate">Invite a friend</span>
        </button>

        {/* Settings — navigates to settings view */}
        <button
          className={cn("nav-item", activeView === "settings" && "active")}
          onClick={() => onViewChange("settings")}
        >
          <span className="flex-shrink-0 opacity-70">
            <Settings size={15} />
          </span>
          <span className="flex-1 truncate">Settings</span>
        </button>

        {/* Help — popover beside the button (no auto-open of mail) */}
        <HelpButton />
      </div>
    </aside>
  );
}

function formatBytes(bytes: number | null | undefined): string {
  if (!bytes || bytes <= 0) return "0 MB";
  const gb = bytes / 1024 ** 3;
  if (gb >= 1) return `${gb.toFixed(gb >= 10 ? 0 : 1)} GB`;
  return `${Math.round(bytes / 1024 ** 2)} MB`;
}

function pct(value: number | null | undefined): string {
  if (value == null || Number.isNaN(value)) return "0%";
  return `${Math.max(0, value).toFixed(value >= 10 ? 0 : 1)}%`;
}

function MetricBar({ value, tone = "primary" }: { value: number; tone?: "primary" | "warn" }) {
  const width = Math.max(2, Math.min(100, value));
  return (
    <div className="h-1.5 rounded-full overflow-hidden" style={{ background: "hsl(var(--surface-4))" }}>
      <div
        className="h-full rounded-full"
        style={{
          width: `${width}%`,
          background: tone === "warn" ? "hsl(38 90% 55%)" : "hsl(var(--primary))",
        }}
      />
    </div>
  );
}

function ProcessLine({ icon, label, process }: {
  icon: React.ReactNode;
  label: string;
  process: ProcessPerf | null;
}) {
  return (
    <div className="rounded-lg px-2.5 py-2" style={{ background: "hsl(var(--surface-3))" }}>
      <div className="flex items-center gap-1.5 mb-1">
        <span className="opacity-70">{icon}</span>
        <span className="text-[10px] font-semibold text-foreground truncate">{label}</span>
        {process && (
          <span className="ml-auto text-[9px] tabular-nums text-muted-foreground">
            {process.pid}
          </span>
        )}
      </div>
      {process ? (
        <div className="grid grid-cols-2 gap-1.5 text-[10px] tabular-nums text-muted-foreground">
          <span>CPU {pct(process.cpu_percent)}</span>
          <span className="text-right">{formatBytes(process.memory_bytes)}</span>
        </div>
      ) : (
        <p className="text-[10px] text-muted-foreground">Not running</p>
      )}
    </div>
  );
}

function PerformanceMonitor() {
  const [sample, setSample] = useState<PerformanceSnapshot | null>(null);
  const [error, setError] = useState(false);

  useEffect(() => {
    let alive = true;
    async function refresh() {
      const next = await getPerformanceSnapshot();
      if (!alive) return;
      if (next) {
        setSample(next);
        setError(false);
      } else {
        setError(true);
      }
    }
    void refresh();
    const timer = window.setInterval(refresh, 1500);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, []);

  const memoryPct = sample && sample.total_memory_bytes > 0
    ? (sample.used_memory_bytes / sample.total_memory_bytes) * 100
    : 0;

  return (
    <div className="rounded-xl glass p-3.5 space-y-3">
      <div className="flex items-center gap-2">
        <Activity size={13} className="text-muted-foreground" />
        <span className="text-[11px] font-semibold text-foreground tracking-wide">
          PERFORMANCE
        </span>
        <span
          className={cn("ml-auto w-1.5 h-1.5 rounded-full", sample && !error && "animate-pulse")}
          style={{ background: error ? "hsl(0 75% 62%)" : "hsl(var(--primary))" }}
        />
      </div>

      {sample ? (
        <>
          <div className="space-y-2">
            <div>
              <div className="flex items-center justify-between text-[10px] tabular-nums mb-1">
                <span className="text-muted-foreground flex items-center gap-1">
                  <Cpu size={10} /> System CPU
                </span>
                <span className="text-foreground font-semibold">{pct(sample.cpu_percent)}</span>
              </div>
              <MetricBar value={sample.cpu_percent} tone={sample.cpu_percent > 75 ? "warn" : "primary"} />
            </div>

            <div>
              <div className="flex items-center justify-between text-[10px] tabular-nums mb-1">
                <span className="text-muted-foreground flex items-center gap-1">
                  <HardDrive size={10} /> Memory
                </span>
                <span className="text-foreground font-semibold">
                  {formatBytes(sample.used_memory_bytes)} / {formatBytes(sample.total_memory_bytes)}
                </span>
              </div>
              <MetricBar value={memoryPct} tone={memoryPct > 80 ? "warn" : "primary"} />
            </div>
          </div>

          <div className="grid gap-1.5">
            <ProcessLine icon={<Server size={10} />} label="Said" process={sample.desktop} />
            <ProcessLine icon={<Cpu size={10} />} label="Backend" process={sample.backend} />
          </div>

          <div className="rounded-lg px-2.5 py-2" style={{ background: "hsl(var(--surface-3))" }}>
            <div className="flex items-center gap-1.5 text-[10px]">
              <Zap size={10} className="text-muted-foreground" />
              <span className="font-semibold text-foreground">GPU</span>
              <span className="ml-auto text-muted-foreground">
                {sample.gpu.available ? pct(sample.gpu.utilization_percent) : "N/A"}
              </span>
            </div>
            <p className="text-[9.5px] text-muted-foreground mt-1 leading-snug">
              {sample.gpu.available ? formatBytes(sample.gpu.memory_bytes) : "GPU metrics not available"}
            </p>
          </div>
        </>
      ) : (
        <p className="text-[10px] text-muted-foreground leading-relaxed">
          {error ? "Monitor unavailable" : "Collecting first sample…"}
        </p>
      )}
    </div>
  );
}

// ── Help popover — anchors to the right of the Help nav item ─────────────────

const SUPPORT_EMAIL = "support@emiactech.com";

function HelpButton() {
  const [open,   setOpen]   = useState(false);
  const [copied, setCopied] = useState(false);
  const btnRef = useRef<HTMLButtonElement>(null);
  const popRef = useRef<HTMLDivElement>(null);
  const [pos,   setPos]     = useState<{ left: number; bottom: number } | null>(null);

  // Reset the "Copied!" affordance whenever the popover reopens
  useEffect(() => { if (!open) setCopied(false); }, [open]);

  async function copyEmail() {
    try {
      await navigator.clipboard.writeText(SUPPORT_EMAIL);
      setCopied(true);
      setTimeout(() => setCopied(false), 1400);
    } catch {
      // Clipboard can fail in restricted contexts — fall back to a hidden
      // textarea + execCommand so we still get something into the buffer.
      const ta = document.createElement("textarea");
      ta.value = SUPPORT_EMAIL;
      ta.style.position = "fixed";
      ta.style.opacity  = "0";
      document.body.appendChild(ta);
      ta.select();
      try { document.execCommand("copy"); } catch {}
      document.body.removeChild(ta);
      setCopied(true);
      setTimeout(() => setCopied(false), 1400);
    }
  }

  // Compute popover position from the anchor button's rect — using a portal
  // so we escape the sidebar's overflow:hidden clipping.
  useLayoutEffect(() => {
    if (!open || !btnRef.current) return;
    const r = btnRef.current.getBoundingClientRect();
    setPos({
      left:   r.right + 10,                          // 10px to the right of the button
      bottom: window.innerHeight - r.bottom,         // align to button's bottom edge
    });
  }, [open]);

  // Click-away + ESC to close
  useEffect(() => {
    if (!open) return;
    function onDoc(e: MouseEvent) {
      const t = e.target as Node;
      if (btnRef.current?.contains(t)) return;
      if (popRef.current?.contains(t)) return;
      setOpen(false);
    }
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") setOpen(false);
    }
    document.addEventListener("mousedown", onDoc);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDoc);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  return (
    <>
      <button
        ref={btnRef}
        className={cn("nav-item", open && "active")}
        onClick={() => setOpen((o) => !o)}
        aria-expanded={open}
      >
        <span className="flex-shrink-0 opacity-70">
          <HelpCircle size={15} />
        </span>
        <span className="flex-1 truncate text-left">Help</span>
      </button>

      {/* Portal to body so overflow:hidden on the sidebar can't clip us,
          and so we sit above any view content (recordings list, etc). */}
      {open && pos && createPortal(
        <div
          ref={popRef}
          className="fixed z-[100] w-[240px] rounded-2xl overflow-hidden"
          style={{
            left: pos.left, bottom: pos.bottom,
            background: "hsl(var(--surface-2))",
            boxShadow:
              "inset 0 0 0 1px hsl(var(--border)), inset 0 1px 0 hsl(0 0% 100% / 0.06), 0 6px 20px hsl(220 60% 2% / 0.30)",
            animation: "fadeIn 0.14s ease-out",
          }}
        >
          {/* Header */}
          <div className="px-4 pt-4 pb-3">
            <div className="flex items-center gap-2 mb-1">
              <span
                className="flex items-center justify-center w-6 h-6 rounded-md flex-shrink-0"
                style={{
                  background: "hsl(var(--primary) / 0.16)",
                  color:      "hsl(var(--primary))",
                }}
              >
                <Mail size={12} />
              </span>
              <span className="text-[13px] font-semibold text-foreground leading-tight">
                Need help?
              </span>
            </div>
            <p className="text-[11.5px] text-muted-foreground leading-relaxed">
              Email us — we usually reply within a day.
            </p>
          </div>

          {/* Email + copy — single click target. Whole row copies the address
              to the clipboard; mailto is intentionally not used because many
              setups (browser-as-default-handler, no mail client configured)
              open an empty Untitled tab instead of a mail composer. */}
          <div className="px-4 pb-4">
            <button
              onClick={copyEmail}
              className="w-full flex items-center justify-between gap-2 px-3 py-2 rounded-lg transition-colors"
              style={{
                background: "hsl(var(--surface-3))",
                boxShadow:  "inset 0 0 0 1px hsl(var(--surface-4))",
              }}
              onMouseEnter={(e) => {
                e.currentTarget.style.background = "hsl(var(--surface-hover))";
              }}
              onMouseLeave={(e) => {
                e.currentTarget.style.background = "hsl(var(--surface-3))";
              }}
            >
              <span
                className="text-[12px] font-medium text-foreground truncate select-text text-left"
                title={SUPPORT_EMAIL}
              >
                {SUPPORT_EMAIL}
              </span>
              <span
                className="flex items-center gap-1 text-[11px] font-semibold flex-shrink-0"
                style={{
                  color: copied ? "hsl(var(--primary))" : "hsl(var(--muted-foreground))",
                }}
              >
                {copied ? (
                  <>
                    <Check size={11} strokeWidth={2.6} />
                    Copied
                  </>
                ) : (
                  <>
                    <Copy size={11} />
                    Copy
                  </>
                )}
              </span>
            </button>
          </div>
        </div>,
        document.body,
      )}
    </>
  );
}
