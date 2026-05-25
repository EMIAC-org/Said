import React, { useEffect, useRef, useState } from "react";
import { Bell, LogOut, User, Sparkles, BookOpen, Star, AlertCircle } from "lucide-react";
import type { AppSnapshot } from "@/types";
import { ThemeToggle } from "@/components/ThemeToggle";
import { BrandMark } from "@/components/BrandMark";
import type { Theme } from "@/lib/useTheme";
import {
  onVocabToast,
  onPendingEditsChanged,
  onVoiceError,
} from "@/lib/invoke";
import { disconnectEnterprise, getConnection } from "@/lib/enterprise";

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

      <div className="max-h-96 overflow-y-auto">
        {entries.length === 0 ? (
          <div className="px-5 py-10 text-center flex flex-col items-center gap-2">
            <BrandMark size={28} idSuffix="notif-empty" className="opacity-50" />
            <p className="text-[13px] text-muted-foreground mt-1">You&apos;re all caught up.</p>
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
  orgName:  string | null;
  avatarUrl: string | null;
  displayName: string | null;
}

function ProfileDropdown({
  info,
  onLogout,
  onClose,
}: {
  info:     ProfileInfo;
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
          className="w-9 h-9 rounded-full flex items-center justify-center flex-shrink-0 overflow-hidden"
          style={{
            background: "hsl(var(--primary) / 0.18)",
            color:      "hsl(var(--primary))",
            boxShadow:  "inset 0 0 0 1px hsl(var(--primary) / 0.30)",
          }}
        >
          {info.avatarUrl ? (
            <img src={info.avatarUrl} alt="" className="w-full h-full object-cover" />
          ) : (
            <User size={14} />
          )}
        </span>
        <div className="flex-1 min-w-0">
          <p className="text-[13px] font-semibold text-foreground leading-tight truncate">
            {info.displayName ?? info.email ?? "Workspace user"}
          </p>
          <p className="text-[11px] text-muted-foreground leading-tight mt-0.5 truncate">
            {info.orgName ?? "Enterprise workspace"}
          </p>
        </div>
      </div>
      <div className="border-t" style={{ borderColor: "hsl(var(--surface-3))" }} />
      <div className="p-1.5">
        <button
          onClick={() => { onClose(); onLogout(); }}
          className="w-full flex items-center gap-2.5 px-3 py-2 text-left text-[12.5px] rounded-lg transition-colors"
          style={{ color: "hsl(0 75% 62%)" }}
          onMouseEnter={(e) => { e.currentTarget.style.background = "hsl(var(--surface-4))"; }}
          onMouseLeave={(e) => { e.currentTarget.style.background = "transparent"; }}
        >
          <LogOut size={13} />
          Disconnect workspace
        </button>
      </div>
    </div>
  );
}

// ── Main Topbar ──────────────────────────────────────────────────────────────

interface TopbarProps {
  snapshot:     AppSnapshot | null;
  theme:        Theme;
  toggleTheme:  () => void;
  onEnterpriseDisconnect?: () => void;
}

const NOTIF_CAP = 50;

export function Topbar({ snapshot: _snapshot, theme, toggleTheme, onEnterpriseDisconnect }: TopbarProps) {
  const [notifs,       setNotifs]       = useState<NotifEntry[]>([]);
  const [notifOpen,    setNotifOpen]    = useState(false);
  const [profileOpen,  setProfileOpen]  = useState(false);
  const [profileInfo,  setProfileInfo]  = useState<ProfileInfo>({
    signedIn: false,
    email: null,
    orgName: null,
    avatarUrl: null,
    displayName: null,
  });

  const refreshProfile = () => {
    const conn = getConnection();
    if (!conn) {
      setProfileInfo({
        signedIn: false,
        email: null,
        orgName: null,
        avatarUrl: null,
        displayName: null,
      });
      return;
    }
    setProfileInfo({
      signedIn: true,
      email: conn.email,
      orgName: conn.orgName ?? null,
      avatarUrl: conn.larkAvatarUrl ?? null,
      displayName: conn.larkName ?? conn.email,
    });
  };

  useEffect(() => { refreshProfile(); }, []);

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

    const unsubPending = onPendingEditsChanged(() => {});

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

  useEffect(() => {
    if (notifOpen && unreadCount > 0) {
      setNotifs((prev) => prev.map((n) => ({ ...n, read: true })));
    }
  }, [notifOpen, unreadCount]);

  const avatarLabel = profileInfo.displayName?.[0]?.toUpperCase()
    ?? profileInfo.email?.[0]?.toUpperCase()
    ?? "U";

  return (
    <header
      className="flex items-center gap-3 h-[var(--topbar-height)] px-5 flex-shrink-0"
      style={{ background: "transparent" }}
    >
      <div data-tauri-drag-region className="flex-1 self-stretch drag-region" />

      <div className="flex items-center gap-2.5 no-drag relative">
        <ThemeToggle theme={theme} toggle={toggleTheme} />

        <div className="relative">
          <button
            onClick={() => { setProfileOpen(false); setNotifOpen((o) => !o); }}
            title="Notifications"
            className="relative w-8 h-8 flex items-center justify-center rounded-full transition-colors"
            style={{
              color:      notifOpen ? "hsl(var(--foreground))" : "hsl(var(--muted-foreground))",
              background: notifOpen ? "hsl(var(--glass-bg))"   : "transparent",
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

        <div className="relative">
          <button
            onClick={() => { setNotifOpen(false); setProfileOpen((o) => !o); }}
            title={profileInfo.displayName ?? profileInfo.email ?? "Profile"}
            className="w-8 h-8 rounded-full flex items-center justify-center text-[11px] font-bold flex-shrink-0 transition-transform overflow-hidden"
            style={{
              background: "hsl(var(--primary) / 0.18)",
              color:      "hsl(var(--primary))",
              boxShadow:  "inset 0 0 0 1px hsl(var(--primary) / 0.30)",
            }}
          >
            {profileInfo.avatarUrl ? (
              <img src={profileInfo.avatarUrl} alt="" className="w-full h-full object-cover" />
            ) : (
              avatarLabel
            )}
          </button>
          {profileOpen && (
            <ProfileDropdown
              info={profileInfo}
              onLogout={async () => {
                await disconnectEnterprise();
                refreshProfile();
                onEnterpriseDisconnect?.();
              }}
              onClose={() => setProfileOpen(false)}
            />
          )}
        </div>
      </div>
    </header>
  );
}
