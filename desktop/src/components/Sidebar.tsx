import React, { useState, useEffect } from "react";
import {
  LayoutDashboard,
  History,
  BarChart2,
  BookOpen,
  Settings,
  UserPlus,
  Bug,
  Activity,
  Cpu,
  HardDrive,
  Server,
  Zap,
  Video,
} from "lucide-react";
import { cn } from "@/lib/utils";
import { BrandMark } from "@/components/BrandMark";
import { getPerformanceSnapshot, openExternal } from "@/lib/invoke";
import {
  getConnection,
  isConnected as isEnterpriseConnected,
} from "@/lib/enterprise";
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
        // Glass sidebar — wallpaper bleeds through softly, deep black tint
        // keeps text readable. Values tuned via the live glass control
        // panel at .context/said-glass-control.html.
        background: "hsl(var(--glass-bg))",
        backdropFilter: "blur(40px) saturate(180%)",
        WebkitBackdropFilter: "blur(40px) saturate(180%)",
        borderRight: "1px solid hsl(var(--glass-stroke))",
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
        <div className="no-drag" title="AirNote — Voice Polish Studio">
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

        {/* Enterprise — Meetings (only when connected) */}
        {isEnterpriseConnected() && (
          <section>
            <p className="section-label px-3 mb-2">Enterprise</p>
            <div className="space-y-0.5">
              <NavButton
                item={{ id: "meetings", label: "Meetings", icon: <Video size={15} /> }}
                isActive={activeView === "meetings"}
                onClick={() => !busy && onViewChange("meetings")}
              />
            </div>
          </section>
        )}

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

        {/* Report bug — opens the control-plane report form with a signed token when connected. */}
        <ReportBugButton snapshot={snapshot} />
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
            <ProcessLine icon={<Server size={10} />} label="AirNote" process={sample.desktop} />
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

// ── Report bug ──────────────────────────────────────────────────────────────

const DEFAULT_REPORT_SERVER = "https://airnote.emiactech.com";

async function getAppVersionForReport(): Promise<string | undefined> {
  try {
    const { getVersion } = await import("@tauri-apps/api/app");
    return await getVersion();
  } catch {
    return undefined;
  }
}

async function getDeviceIdForReport(): Promise<string | undefined> {
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    return await invoke<string>("get_device_id");
  } catch {
    return undefined;
  }
}

function buildPublicReportUrl(base: string, params: Record<string, string | undefined>): string {
  const url = new URL("/report-bug", base);
  for (const [key, value] of Object.entries(params)) {
    if (value?.trim()) url.searchParams.set(key, value.trim());
  }
  return url.toString();
}

function ReportBugButton({ snapshot }: { snapshot: AppSnapshot | null }) {
  const [opening, setOpening] = useState(false);

  async function openReport() {
    if (opening) return;
    setOpening(true);
    try {
      const conn = getConnection();
      const base = (conn?.serverUrl || DEFAULT_REPORT_SERVER).replace(/\/+$/, "");
      const [appVersion, deviceId] = await Promise.all([
        getAppVersionForReport(),
        getDeviceIdForReport(),
      ]);
      const context = {
        app_version: appVersion,
        platform: snapshot?.platform || undefined,
        device_id: deviceId,
      };
      let url = buildPublicReportUrl(base, context);

      if (conn?.jwt) {
        try {
          const res = await fetch(`${base}/v1/bug-reports/session`, {
            method: "POST",
            headers: {
              Authorization: `Bearer ${conn.jwt}`,
              "Content-Type": "application/json",
            },
            body: JSON.stringify(context),
          });
          if (res.ok) {
            const data = await res.json();
            const returned = typeof data.url === "string" ? data.url : data.path;
            if (typeof returned === "string" && returned.trim()) {
              url = new URL(returned, base).toString();
            }
          }
        } catch {
          // Fall back to the public page; it will explain that sign-in is needed.
        }
      }

      await openExternal(url);
    } finally {
      setOpening(false);
    }
  }

  return (
    <button
      className={cn("nav-item", opening && "disabled")}
      onClick={() => void openReport()}
      disabled={opening}
    >
      <span className="flex-shrink-0 opacity-70">
        <Bug size={15} />
      </span>
      <span className="flex-1 truncate text-left">
        {opening ? "Opening…" : "Report bug"}
      </span>
    </button>
  );
}
