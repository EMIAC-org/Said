import React, { useEffect, useRef, useState } from "react";
import { Bell, LogOut, LogIn, User, Sparkles, BookOpen, Star, AlertCircle } from "lucide-react";
import type { AppSnapshot } from "@/types";
import { ThemeToggle } from "@/components/ThemeToggle";
import { BrandMark } from "@/components/BrandMark";
import type { Theme } from "@/lib/useTheme";
import {
  cloudLogout,
  getCloudStatus,
  onVocabToast,
  onPendingEditsChanged,
  onVoiceError,
} from "@/lib/invoke";

// ── Notification log entry ───────────────────────────────────────────────────

interface NotifEntry {
  id:        string;
  kind:      "vocab-added" | "vocab-removed" | "vocab-starred" | "error" | "info";
  title:     string;
  body:      string;
  timestamp: number;       // ms
  read:      boolean;
}

// ── Helpers ──────────────────────────────────────────────────────────────────

function formatRelative(ts: number): string {
  const sec = Math.max(1, Math.floor((Date.now() - ts) / 1000));
  if (sec < 60) return `${sec}s ago`;
  const min = Math.floor(sec / 60);
  if (min < 60) return `${min}m ago`;
  const hr = Math.floor(min / 60);
  if (hr < 24) return `${hr}h ago`;
  const day = Math.floor(hr / 24);
  return `${day}d ago`;
}

// ── Notification dropdown ────────────────────────────────────────────────────

function NotifDropdown({
  entries,
  onClear,
  onClose,
}: {
  entries: NotifEntry[];
  onClear: () => void;
  onClose: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const onDoc = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose();
    };
    document.addEventListener("mousedown", onDoc);
    return () => document.removeEventListener("mousedown", onDoc);
  }, [onClose]);

  return (
    <div
      ref={ref}
      className="absolute right-0 top-10 z-50 w-80 rounded-2xl shadow-xl overflow-hidden"
      style={{
        background: "hsl(var(--surface-3))",
        border: "1px solid hsl(var(--border))",
        boxShadow: "0 12px 40px rgba(0,0,0,0.30)",
        animation: "fadeIn 0.15s ease-out",
      }}
    >
      {/* Header — Said brand mark + label */}
      <div
        className="flex items-center justify-between px-4 py-3 border-b"
        style={{ borderColor: "hsl(var(--surface-3))" }}
      >
        <div className="flex items-center gap-2">
          <BrandMark size={18} idSuffix="notif-header" />
          <span className="text-[12px] font-bold uppercase tracking-[0.12em] text-muted-foreground">
            Notifications
          </span>
        </div>
        {entries.length > 0 && (
          <button
            onClick={onClear}
            className="text-[11px] text-muted-foreground hover:text-foreground transition-colors"
          >
            Clear all
          </button>
        )}
      </div>

      {/* List */}
      <div className="max-h-96 overflow-y-auto">
        {entries.length === 0 ? (
          <div className="px-5 py-10 text-center flex flex-col items-center gap-2">
            <BrandMark size={28} idSuffix="notif-empty" className="opacity-50" />
            <p className="text-[13px] text-muted-foreground mt-1">You're all caught up.</p>
            <p className="text-[11px] text-muted-foreground opacity-70 max-w-[220px] leading-relaxed">
              Learning updates and recording issues will land here.
            </p>
          </div>
        ) : (
          entries.map((n, idx) => (
            <React.Fragment key={n.id}>
              {idx > 0 && (
                <div className="mx-4 border-t" style={{ borderColor: "hsl(var(--surface-3))" }} />
              )}
              <div className="flex items-start gap-3 px-4 py-3">
                <span
                  className="w-7 h-7 rounded-full flex items-center justify-center flex-shrink-0 mt-0.5"
                  style={{
                    background:
                      n.kind === "error"
                        ? "hsl(0 70% 60% / 0.16)"
                        : n.kind === "vocab-starred"
                        ? "hsl(var(--chip-amber-bg))"
                        : n.kind === "vocab-removed"
                        ? "hsl(var(--surface-4))"
                        : "hsl(var(--chip-mint-bg))",
                    color:
                      n.kind === "error"
                        ? "hsl(0 70% 60%)"
                        : n.kind === "vocab-starred"
                        ? "hsl(var(--chip-amber-fg))"
                        : n.kind === "vocab-removed"
                        ? "hsl(var(--muted-foreground))"
                        : "hsl(var(--chip-mint-fg))",
                  }}
                >
                  {n.kind === "error" ? (
                    <AlertCircle size={11} strokeWidth={2.4} />
                  ) : n.kind === "vocab-starred" ? (
                    <Star size={11} fill="currentColor" />
                  ) : n.kind === "vocab-removed" ? (
                    <BookOpen size={11} />
                  ) : (
                    <Sparkles size={11} />
                  )}
                </span>
                <div className="flex-1 min-w-0">
                  <p className="text-[12.5px] font-semibold text-foreground leading-tight">
                    {n.title}
                  </p>
                  <p className="text-[11.5px] text-muted-foreground leading-snug mt-0.5">
                    {n.body}
                  </p>
                  <p className="text-[10px] text-muted-foreground mt-1 tabular-nums">
                    {formatRelative(n.timestamp)}
                  </p>
                </div>
              </div>
            </React.Fragment>
          ))
        )}
      </div>
    </div>
  );
}

// ── Profile dropdown ─────────────────────────────────────────────────────────

interface ProfileInfo {
  signedIn: boolean;
  email:    string | null;
}

function ProfileDropdown({
  info,
  onLogin,
  onLogout,
  onClose,
}: {
  info:     ProfileInfo;
  onLogin:  () => void;
  onLogout: () => void;
  onClose:  () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const onDoc = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose();
    };
    document.addEventListener("mousedown", onDoc);
    return () => document.removeEventListener("mousedown", onDoc);
  }, [onClose]);

  return (
    <div
      ref={ref}
      className="absolute right-0 top-10 z-50 w-64 rounded-2xl shadow-xl overflow-hidden"
      style={{
        background: "hsl(var(--surface-3))",
        border: "1px solid hsl(var(--border))",
        boxShadow: "0 12px 40px rgba(0,0,0,0.30)",
        animation: "fadeIn 0.15s ease-out",
      }}
    >
      <div className="px-4 py-3.5 flex items-center gap-3">
        <span
          className="w-9 h-9 rounded-full flex items-center justify-center flex-shrink-0"
          style={{
            background: "hsl(var(--primary) / 0.18)",
            color:      "hsl(var(--primary))",
            boxShadow:  "inset 0 0 0 1px hsl(var(--primary) / 0.30)",
          }}
        >
          <User size={14} />
        </span>
        <div className="flex-1 min-w-0">
          <p className="text-[13px] font-semibold text-foreground leading-tight truncate">
            {info.signedIn ? (info.email ?? "Signed in") : "Guest"}
          </p>
          <p className="text-[11px] text-muted-foreground leading-tight mt-0.5 truncate">
            {info.signedIn ? "Signed in to AirNote" : "Not signed in"}
          </p>
        </div>
      </div>
      <div className="border-t" style={{ borderColor: "hsl(var(--surface-3))" }} />
      <div className="p-1.5">
        {info.signedIn ? (
          <button
            onClick={() => { onClose(); onLogout(); }}
            className="w-full flex items-center gap-2.5 px-3 py-2 text-left text-[12.5px] rounded-lg transition-colors"
            style={{ color: "hsl(0 75% 62%)" }}
            onMouseEnter={(e) => { e.currentTarget.style.background = "hsl(var(--surface-4))"; }}
            onMouseLeave={(e) => { e.currentTarget.style.background = "transparent"; }}
          >
            <LogOut size={13} />
            Sign out
          </button>
        ) : (
          <button
            onClick={() => { onClose(); onLogin(); }}
            className="w-full flex items-center gap-2.5 px-3 py-2 text-left text-[12.5px] rounded-lg transition-colors text-foreground"
            onMouseEnter={(e) => { e.currentTarget.style.background = "hsl(var(--surface-4))"; }}
            onMouseLeave={(e) => { e.currentTarget.style.background = "transparent"; }}
          >
            <LogIn size={13} />
            Sign in or create an account
          </button>
        )}
      </div>
    </div>
  );
}

// ── Main Topbar ──────────────────────────────────────────────────────────────

interface TopbarProps {
  snapshot:     AppSnapshot | null;
  theme:        Theme;
  toggleTheme:  () => void;
  onLoginClick?: () => void;
}

const NOTIF_CAP = 50;

export function Topbar({ snapshot: _snapshot, theme, toggleTheme, onLoginClick }: TopbarProps) {
  const [notifs,       setNotifs]       = useState<NotifEntry[]>([]);
  const [notifOpen,    setNotifOpen]    = useState(false);
  const [profileOpen,  setProfileOpen]  = useState(false);
  const [profileInfo,  setProfileInfo]  = useState<ProfileInfo>({ signedIn: false, email: null });

  // ── Refresh profile/auth state ───────────────────────────────────────────
  const refreshProfile = async () => {
    try {
      const status = await getCloudStatus();
      setProfileInfo({
        signedIn: status?.connected === true,
        email:    status?.email ?? null,
      });
    } catch {
      setProfileInfo({ signedIn: false, email: null });
    }
  };
  useEffect(() => { refreshProfile(); }, []);

  // ── Subscribe to vocab + pending events to populate notification log ─────
  useEffect(() => {
    const push = (e: Omit<NotifEntry, "id" | "timestamp" | "read">) => {
      setNotifs((prev) => {
        const next: NotifEntry = {
          ...e,
          id:        `${Date.now()}-${Math.random().toString(36).slice(2, 7)}`,
          timestamp: Date.now(),
          read:      false,
        };
        return [next, ...prev].slice(0, NOTIF_CAP);
      });
    };

    const unsubVocab = onVocabToast((payload) => {
      // "added" is handled by the in-app VocabularyToast — don't duplicate in bell
      if (payload.kind === "starred") {
        push({
          kind:  "vocab-starred",
          title: "Pinned to vocabulary",
          body:  `AirNote will keep "${payload.term}" even if you stop using it.`,
        });
      } else if (payload.kind === "removed") {
        push({
          kind:  "vocab-removed",
          title: "Removed from vocabulary",
          body:  `AirNote won't recognise "${payload.term}" any more.`,
        });
      } else if (payload.kind === "queued") {
        push({
          kind:  "info",
          title: "Noticed your correction",
          body:  `Make this fix once more and AirNote will remember "${payload.term}".`,
        });
      }
    });

    // Listen for pending-edits-changed as a soft signal too (no spam — only
    // fires once per learning event).
    const unsubPending = onPendingEditsChanged(() => {
      // Intentional no-op; the vocab-toast events above carry the user-facing
      // message.  Hook kept so we can extend later (e.g., "1 edit awaiting
      // review" inside the panel).
    });

    // Voice errors (empty recording, transcription failures, etc.) — these
    // already fire a transient retry toast; mirror them into the in-app log
    // so the user sees a history when they open the bell.
    const unsubError = onVoiceError((message, audioId) => {
      const empty = /no\s*(speech|audio)|empty|too short/i.test(message);
      push({
        kind:  "error",
        title: empty ? "Nothing recorded" : "Recording didn't make it",
        body:  empty
          ? "We didn't catch any speech. Try again — speak a little closer to the mic."
          : message || (audioId ? "We saved the audio so you can retry it." : "Try again in a moment."),
      });
    });

    return () => { unsubVocab(); unsubPending(); unsubError(); };
  }, []);

  const unreadCount = notifs.filter((n) => !n.read).length;

  // Mark all read when dropdown opens
  useEffect(() => {
    if (notifOpen && unreadCount > 0) {
      setNotifs((prev) => prev.map((n) => ({ ...n, read: true })));
    }
  }, [notifOpen, unreadCount]);

  return (
    <header
      className="flex items-center gap-3 h-[var(--topbar-height)] px-5 flex-shrink-0"
      style={{ background: "transparent" }}
    >
      {/* Drag-region spacer — search now lives only in the History view. */}
      <div data-tauri-drag-region className="flex-1 self-stretch drag-region" />

      {/* Right actions — no-drag region */}
      <div className="flex items-center gap-2.5 no-drag relative">
        {/* Theme toggle */}
        <ThemeToggle theme={theme} toggle={toggleTheme} />

        {/* Notification bell ────────────────────────── */}
        <div className="relative">
          <button
            onClick={() => { setProfileOpen(false); setNotifOpen((o) => !o); }}
            title="Notifications"
            className="relative w-8 h-8 flex items-center justify-center rounded-full transition-colors"
            style={{
              color:      notifOpen ? "hsl(var(--foreground))" : "hsl(var(--muted-foreground))",
              background: notifOpen ? "hsl(var(--glass-bg))"   : "transparent",
            }}
            onMouseEnter={(e) => {
              if (!notifOpen) {
                e.currentTarget.style.background = "hsl(var(--glass-bg))";
                e.currentTarget.style.color      = "hsl(var(--foreground))";
              }
            }}
            onMouseLeave={(e) => {
              if (!notifOpen) {
                e.currentTarget.style.background = "transparent";
                e.currentTarget.style.color      = "hsl(var(--muted-foreground))";
              }
            }}
          >
            <Bell size={14} />
            {unreadCount > 0 && (
              <span
                className="absolute -top-0.5 -right-0.5 min-w-[16px] h-4 px-1 rounded-full text-[9px] font-bold flex items-center justify-center tabular-nums"
                style={{
                  background: "hsl(0 70% 60%)",
                  color:      "white",
                  boxShadow:  "0 0 0 2px hsl(var(--surface-1))",
                }}
              >
                {unreadCount > 9 ? "9+" : unreadCount}
              </span>
            )}
          </button>
          {notifOpen && (
            <NotifDropdown
              entries={notifs}
              onClear={() => setNotifs([])}
              onClose={() => setNotifOpen(false)}
            />
          )}
        </div>

        {/* Profile avatar ─────────────────────────── */}
        <div className="relative">
          <button
            onClick={() => { setNotifOpen(false); setProfileOpen((o) => !o); }}
            title={profileInfo.signedIn ? (profileInfo.email ?? "Signed in") : "Guest"}
            className="w-8 h-8 rounded-full flex items-center justify-center text-[11px] font-bold flex-shrink-0 transition-transform"
            style={{
              background: "hsl(var(--primary) / 0.18)",
              color:      "hsl(var(--primary))",
              boxShadow:  "inset 0 0 0 1px hsl(var(--primary) / 0.30)",
            }}
            onMouseEnter={(e) => { e.currentTarget.style.transform = "scale(1.05)"; }}
            onMouseLeave={(e) => { e.currentTarget.style.transform = "scale(1)"; }}
          >
            {profileInfo.signedIn
              ? (profileInfo.email?.[0]?.toUpperCase() ?? "U")
              : "G"}
          </button>
          {profileOpen && (
            <ProfileDropdown
              info={profileInfo}
              onLogin={() => onLoginClick?.()}
              onLogout={async () => {
                try { await cloudLogout(); } catch { /* non-critical */ }
                await refreshProfile();
              }}
              onClose={() => setProfileOpen(false)}
            />
          )}
        </div>
      </div>
    </header>
  );
}
