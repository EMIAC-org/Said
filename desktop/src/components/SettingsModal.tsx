import React, { useEffect, useRef, useState } from "react";
import {
  X, RefreshCw, CloudCheck,
  Wand2, ShieldCheck, Key, Info, Bug, Palette, Link, Bell, Brain, Keyboard,
} from "lucide-react";
import { getVersion } from "@tauri-apps/api/app";
import {
  SettingsView,
  SETTINGS_SECTIONS,
  type SettingsSection,
} from "@/components/views/SettingsView";
import type { AppSnapshot } from "@/types";

/* ════════════════════════════════════════════════════════════════════════════
   SettingsModal — two-pane modal mirroring the InviteTeamModal aesthetic.
   Left nav: scoped Settings sections only. Right pane renders the matching
   SettingsView section.
   ════════════════════════════════════════════════════════════════════════════ */

const SECTION_ICONS: Record<SettingsSection, React.ReactNode> = {
  "appearance":    <Palette      size={14} />,
  "writing":       <Wand2        size={14} />,
  "hotkeys":       <Keyboard     size={14} />,
  "models":        <Brain        size={14} />,
  "notifications": <Bell         size={14} />,
  "permissions":   <ShieldCheck  size={14} />,
  "api-keys":    <Key          size={14} />,
  "enterprise":  <Link         size={14} />,
  "debug":       <Bug          size={14} />,
  "about":       <Info         size={14} />,
};

const SECTION_TITLES: Record<SettingsSection, string> = {
  "appearance":    "Appearance",
  "writing":       "Writing style",
  "hotkeys":       "Hotkeys",
  "models":        "Models",
  "notifications": "Notifications",
  "permissions":   "Permissions",
  "api-keys":    "API keys",
  "enterprise":  "Enterprise",
  "debug":       "Debug",
  "about":       "About",
};

const SECTION_SUBTITLES: Record<SettingsSection, string> = {
  "appearance":    "Choose how the dashboard surfaces your activity.",
  "writing":       "Advanced voice prompt controls.",
  "hotkeys":       "Choose the key AirNote listens for while you speak.",
  "models":        "Dictation speed, quality, and ChatGPT connection.",
  "notifications": "Control which status bar alerts you see.",
  "permissions":   "Accessibility, input monitoring, notifications.",
  "api-keys":    "Groq and Deepgram keys (stored locally).",
  "enterprise":  "Connect to your organization's AirNote Enterprise server.",
  "debug":       "Recent app and backend logs.",
  "about":       "Version and credits.",
};

interface Props {
  open:               boolean;
  onClose:            () => void;
  snapshot:           AppSnapshot | null;
  onAccessibility:    () => void;
  onInputMonitoring:  () => void;
  onMicrophone:       () => void;
  performanceMonitorEnabled?: boolean;
  onPerformanceMonitorChange?: (enabled: boolean) => void;
  onEnterpriseDisconnect?: () => void;
  /** Optional initial section to land on. Defaults to "models". */
  initialSection?:    SettingsSection;
}

export function SettingsModal({
  open, onClose, snapshot, onAccessibility, onInputMonitoring,
  onMicrophone, performanceMonitorEnabled, onPerformanceMonitorChange,
  onEnterpriseDisconnect, initialSection,
}: Props) {
  const [activeSection, setActiveSection] = useState<SettingsSection>(
    initialSection ?? "models"
  );
  const [appVersion, setAppVersion] = useState("…");
  const dialogRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    getVersion().then(setAppVersion).catch(() => setAppVersion("?"));
  }, []);

  // ESC closes
  useEffect(() => {
    if (!open) return;
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") onClose();
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  // Reset to initial section when reopened
  useEffect(() => {
    if (open) setActiveSection(initialSection ?? "models");
  }, [open, initialSection]);

  if (!open) return null;

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center"
      style={{
        background: "hsl(var(--foreground) / 0.22)",
        backdropFilter: "blur(8px)",
        WebkitBackdropFilter: "blur(8px)",
        animation: "fadeIn 0.18s ease-out",
      }}
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        ref={dialogRef}
        className="rounded-[20px] overflow-hidden flex"
        style={{
          background: "hsl(var(--surface-2))",
          width:  "min(1000px, 94vw)",
          height: "min(680px, 92vh)",
          boxShadow: "var(--shadow-pop)",
        }}
      >

        {/* ─────────── LEFT NAV ─────────── */}
        <aside
          className="flex flex-col flex-shrink-0"
          style={{
            width: 240,
            background: "hsl(var(--surface-1))",
            borderRight: "1px solid hsl(var(--surface-4))",
          }}
        >
          {/* Top brand row */}
          <div className="px-5 py-4 flex items-center justify-between"
               style={{ borderBottom: "1px solid hsl(var(--surface-4))" }}>
            <p className="section-label flex items-center gap-2">
              <span
                className="inline-block w-1 h-1 rounded-full"
                style={{ background: "hsl(var(--accent-violet))" }}
              />
              Settings
            </p>
          </div>

          {/* Section nav */}
          <nav className="flex-1 overflow-y-auto px-3 pt-3 pb-3">
            <div className="space-y-0.5">
              {SETTINGS_SECTIONS.map((s) => {
                const active = s.id === activeSection;
                return (
                  <button
                    key={s.id}
                    onClick={() => setActiveSection(s.id)}
                    className="w-full flex items-center gap-2.5 px-3 py-2 rounded-xl text-[13px] font-medium transition-all text-left"
                    style={{
                      background: active ? "hsl(var(--surface-3))" : "transparent",
                      color:      active ? "hsl(var(--foreground))" : "hsl(var(--muted-foreground))",
                      fontWeight: active ? 600 : 500,
                      boxShadow:  active
                        ? "inset 0 0 0 1px hsl(var(--glass-stroke))"
                        : "none",
                    }}
                    onMouseEnter={(e) => {
                      if (!active) {
                        e.currentTarget.style.background = "hsl(var(--surface-hover))";
                        e.currentTarget.style.color      = "hsl(var(--foreground))";
                      }
                    }}
                    onMouseLeave={(e) => {
                      if (!active) {
                        e.currentTarget.style.background = "transparent";
                        e.currentTarget.style.color      = "hsl(var(--muted-foreground))";
                      }
                    }}
                  >
                    <span className="flex-shrink-0 opacity-70">{SECTION_ICONS[s.id]}</span>
                    <span className="flex-1 truncate">{s.label}</span>
                  </button>
                );
              })}
            </div>
          </nav>

          {/* Footer — version + sync indicator */}
          <div
            className="px-4 py-3 flex items-center justify-between flex-shrink-0"
            style={{ borderTop: "1px solid hsl(var(--surface-4))" }}
          >
            <p className="text-[11px] tabular-nums"
               style={{ color: "hsl(var(--muted-foreground))" }}>
              AirNote v{appVersion}
            </p>
            <span
              className="flex items-center justify-center w-5 h-5 rounded-full"
              style={{
                color: "hsl(var(--primary))",
                background: "hsl(var(--primary) / 0.14)",
              }}
              title="Preferences synced locally"
            >
              <CloudCheck size={11} />
            </span>
          </div>
        </aside>

        {/* ─────────── RIGHT PANE ─────────── */}
        <main className="flex-1 flex flex-col min-w-0 relative overflow-hidden">

          {/* Subtle violet wash top-right */}
          <div
            aria-hidden
            className="absolute pointer-events-none"
            style={{
              right: -120, top: -120, width: 320, height: 320, borderRadius: "50%",
              background: "radial-gradient(circle, hsl(var(--accent-violet) / 0.10) 0%, transparent 70%)",
            }}
          />

          {/* Header */}
          <header
            className="relative flex items-start justify-between px-7 py-5"
            style={{ borderBottom: "1px solid hsl(var(--surface-4))" }}
          >
            <div>
              <h2
                className="text-[22px] font-extrabold tracking-tight leading-none"
                style={{
                  color: "hsl(var(--foreground))",
                  letterSpacing: "-0.02em",
                }}
              >
                {SECTION_TITLES[activeSection]}
              </h2>
              <p className="text-[12.5px] mt-2"
                 style={{ color: "hsl(var(--muted-foreground))" }}>
                {SECTION_SUBTITLES[activeSection]}
              </p>
            </div>
            <div className="flex items-center gap-2 flex-shrink-0">
              <button
                title="Refresh"
                className="w-8 h-8 rounded-full flex items-center justify-center transition-colors"
                style={{ color: "hsl(var(--muted-foreground))" }}
                onMouseEnter={(e) => {
                  e.currentTarget.style.background = "hsl(var(--surface-4))";
                  e.currentTarget.style.color      = "hsl(var(--foreground))";
                }}
                onMouseLeave={(e) => {
                  e.currentTarget.style.background = "transparent";
                  e.currentTarget.style.color      = "hsl(var(--muted-foreground))";
                }}
              >
                <RefreshCw size={14} />
              </button>
              <button
                onClick={onClose}
                title="Close"
                className="w-8 h-8 rounded-full flex items-center justify-center transition-colors"
                style={{ color: "hsl(var(--muted-foreground))" }}
                onMouseEnter={(e) => {
                  e.currentTarget.style.background = "hsl(var(--surface-4))";
                  e.currentTarget.style.color      = "hsl(var(--foreground))";
                }}
                onMouseLeave={(e) => {
                  e.currentTarget.style.background = "transparent";
                  e.currentTarget.style.color      = "hsl(var(--muted-foreground))";
                }}
              >
                <X size={14} />
              </button>
            </div>
          </header>

          {/* Body — scoped SettingsView, embedded mode */}
          <div className="relative flex-1 overflow-y-auto px-7 py-5">
            <SettingsView
              snapshot={snapshot}
              onAccessibility={onAccessibility}
              onInputMonitoring={onInputMonitoring}
              onMicrophone={onMicrophone}
              performanceMonitorEnabled={performanceMonitorEnabled}
              onPerformanceMonitorChange={onPerformanceMonitorChange}
              onEnterpriseDisconnect={onEnterpriseDisconnect}
              activeSection={activeSection}
              hideHeader
              embedded
            />
          </div>
        </main>
      </div>
    </div>
  );
}
