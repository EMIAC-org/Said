import { useEffect, useState, useCallback } from "react";
import { Video, Users, Plus, RefreshCw, Calendar, Loader2 } from "lucide-react";
import { getConnection, listMeetings } from "@/lib/enterprise";
import { openExternal } from "@/lib/invoke";

// ── Types ─────────────────────────────────────────────────────────────────────

interface Meeting {
  id: string;
  title: string;
  status: "scheduled" | "live" | "ended";
  scheduled_at?: string | null;
  agenda?: string | null;
  participants_count?: number;
  created_at: string;
}

// ── Status badge ──────────────────────────────────────────────────────────────

function StatusBadge({ status }: { status: Meeting["status"] }) {
  const config: Record<string, { bg: string; fg: string; label: string }> = {
    live:      { bg: "hsl(142 70% 45% / 0.14)", fg: "hsl(142 70% 65%)", label: "Live" },
    scheduled: { bg: "hsl(var(--chip-blue-bg))",  fg: "hsl(var(--chip-blue-fg))", label: "Scheduled" },
    ended:     { bg: "hsl(240 5% 40% / 0.14)",   fg: "hsl(240 5% 60%)",          label: "Ended" },
  };
  const s = config[status] ?? config.ended;

  return (
    <span
      className="inline-flex items-center gap-1.5 text-[10px] font-bold uppercase tracking-wide px-2 py-0.5 rounded-md flex-shrink-0"
      style={{ background: s.bg, color: s.fg }}
    >
      {status === "live" && (
        <span
          className="w-1.5 h-1.5 rounded-full animate-pulse"
          style={{
            background: "hsl(142 70% 55%)",
            boxShadow: "0 0 6px hsl(142 70% 55% / 0.6)",
          }}
        />
      )}
      {s.label}
    </span>
  );
}

// ── Meeting card ──────────────────────────────────────────────────────────────

function MeetingCard({ meeting, onJoinMeeting }: { meeting: Meeting; onJoinMeeting?: (id: string) => void }) {
  const time = (() => {
    const raw = meeting.scheduled_at ?? meeting.created_at;
    if (!raw) return "No time set";
    try {
      return new Date(raw).toLocaleString(undefined, {
        weekday: "short",
        month: "short",
        day: "numeric",
        hour: "numeric",
        minute: "2-digit",
      });
    } catch {
      return "No time set";
    }
  })();

  const isJoinable = meeting.status === "live" || meeting.status === "scheduled";

  return (
    <div
      className={`rounded-xl p-4 transition-colors ${isJoinable ? "cursor-pointer hover:brightness-110" : ""}`}
      style={{
        background: "hsl(var(--surface-3))",
        boxShadow: "inset 0 0 0 1px hsl(var(--surface-4))",
      }}
      onClick={() => isJoinable && onJoinMeeting?.(meeting.id)}
    >
      <div className="flex items-start justify-between gap-3">
        <div className="flex-1 min-w-0">
          <h3 className="text-[13px] font-semibold text-foreground truncate">
            {meeting.title}
          </h3>
          <p className="text-[11px] text-muted-foreground mt-1 flex items-center gap-1.5">
            <Calendar size={10} className="flex-shrink-0" />
            {time}
          </p>
        </div>
        <StatusBadge status={meeting.status} />
      </div>

      {meeting.participants_count != null && meeting.participants_count > 0 && (
        <div className="flex items-center gap-1.5 mt-3 text-[11px] text-muted-foreground">
          <Users size={11} className="flex-shrink-0" />
          <span>
            {meeting.participants_count} participant
            {meeting.participants_count !== 1 ? "s" : ""}
          </span>
        </div>
      )}
    </div>
  );
}

// ── Main view ─────────────────────────────────────────────────────────────────

interface MeetingsViewProps {
  onJoinMeeting?: (meetingId: string) => void;
}

export function MeetingsView({ onJoinMeeting }: MeetingsViewProps) {
  const [meetings, setMeetings] = useState<Meeting[]>([]);
  const [loading, setLoading]   = useState(true);
  const [error, setError]       = useState("");

  const fetchMeetings = useCallback(async () => {
    const conn = getConnection();
    if (!conn) {
      setError("Not connected to enterprise server");
      setLoading(false);
      return;
    }
    setLoading(true);
    setError("");
    try {
      const result = await listMeetings(conn.serverUrl, conn.jwt);
      setMeetings(result as Meeting[]);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load meetings");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    fetchMeetings();
    // Poll every 15 seconds so live status stays fresh
    const interval = setInterval(fetchMeetings, 15_000);
    return () => clearInterval(interval);
  }, [fetchMeetings]);

  const handleNewMeeting = () => {
    const conn = getConnection();
    if (!conn) return;
    const adminUrl = `${conn.serverUrl}/admin/meetings`;
    openExternal(adminUrl);
  };

  // Sort: live first, then scheduled, then ended
  const sortedMeetings = [...meetings].sort((a, b) => {
    const order: Record<string, number> = { live: 0, scheduled: 1, ended: 2 };
    return (order[a.status] ?? 2) - (order[b.status] ?? 2);
  });

  return (
    <div className="h-full flex flex-col overflow-hidden">
      {/* Header */}
      <div className="flex items-center justify-between px-6 pt-5 pb-4 flex-shrink-0">
        <div className="flex items-center gap-3">
          <div
            className="flex items-center justify-center w-8 h-8 rounded-lg"
            style={{ background: "hsl(var(--primary) / 0.12)" }}
          >
            <Video size={16} style={{ color: "hsl(var(--primary))" }} />
          </div>
          <div>
            <h1 className="text-[16px] font-bold text-foreground">Meetings</h1>
            <p className="text-[11px] text-muted-foreground">
              Your scheduled and live meetings
            </p>
          </div>
        </div>

        <div className="flex items-center gap-2">
          <button
            onClick={() => void fetchMeetings()}
            disabled={loading}
            className="flex items-center justify-center w-8 h-8 rounded-lg transition-colors"
            style={{ background: "hsl(var(--surface-3))" }}
            title="Refresh"
          >
            <RefreshCw
              size={13}
              className={loading ? "animate-spin" : ""}
              style={{ color: "hsl(var(--muted-foreground))" }}
            />
          </button>
          <button
            onClick={handleNewMeeting}
            className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-[12px] font-semibold transition-colors"
            style={{
              background: "hsl(var(--primary))",
              color: "hsl(var(--primary-foreground))",
            }}
          >
            <Plus size={13} />
            New Meeting
          </button>
        </div>
      </div>

      {/* Content */}
      <div className="flex-1 overflow-y-auto px-6 pb-6">
        {loading && meetings.length === 0 ? (
          <div className="flex flex-col items-center justify-center h-full gap-3 opacity-60">
            <Loader2
              size={20}
              className="animate-spin text-muted-foreground"
            />
            <p className="text-[12px] text-muted-foreground">
              Loading meetings...
            </p>
          </div>
        ) : error ? (
          <div className="flex flex-col items-center justify-center h-full gap-3">
            <p className="text-[12px] text-muted-foreground">{error}</p>
            <button
              onClick={() => void fetchMeetings()}
              className="text-[11px] font-medium px-3 py-1.5 rounded-lg transition-colors"
              style={{
                background: "hsl(var(--surface-3))",
                color: "hsl(var(--foreground))",
              }}
            >
              Retry
            </button>
          </div>
        ) : sortedMeetings.length === 0 ? (
          <div className="flex flex-col items-center justify-center h-full gap-3 opacity-60">
            <Video size={28} className="text-muted-foreground" />
            <p className="text-[12px] text-muted-foreground">
              No meetings yet
            </p>
            <p className="text-[11px] text-muted-foreground">
              Create one from your admin dashboard
            </p>
          </div>
        ) : (
          <div className="space-y-2">
            {sortedMeetings.map((meeting) => (
              <MeetingCard key={meeting.id} meeting={meeting} onJoinMeeting={onJoinMeeting} />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
