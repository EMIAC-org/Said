import { useEffect, useState, useRef, useCallback } from "react";
import {
  ArrowLeft,
  Radio,
  Users,
  Check,
  Send,
  Loader2,
  Wifi,
  WifiOff,
  Mic,
  MicOff,
  PhoneOff,
  CircleStop,
} from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getConnection } from "@/lib/enterprise";

// ── Types (matching WS protocol) ─────────────────────────────────────────────

interface TranscriptChunk {
  speaker_id: string;
  speaker_name: string;
  text: string;
  timestamp_ms: number;
  chunk_index: number;
}

interface WsTask {
  task_id: string;
  title: string;
  assignee_name: string | null;
  lark_task_id?: string | null;
  status?: string;
}

interface WsParticipant {
  account_id: string;
  email: string;
  display_name?: string;
  status: string;
}

interface MeetingDetail {
  id: string;
  title: string;
  status: "scheduled" | "live" | "ended";
  created_by: string;
}

interface MeetingSttStatus {
  active: boolean;
  muted: boolean;
  capture_running: boolean;
}

type WsMessage =
  | { type: "connected"; meeting_id: string; account_id: string }
  | { type: "catchup"; summary: string | null; current_chunks: TranscriptChunk[]; tasks: WsTask[]; decisions: { text: string }[]; participants: WsParticipant[] }
  | { type: "transcript_chunk"; speaker_id: string; speaker_name: string; text: string; timestamp_ms: number; chunk_index: number }
  | { type: "task_detected"; task_id: string; title: string; assignee_id: string | null; assignee_name: string | null }
  | { type: "summary_updated"; summary: string }
  | { type: "participant_joined"; account_id: string; email: string; display_name?: string }
  | { type: "participant_left"; account_id: string }
  | { type: "meeting_ended"; meeting_id: string };

// ── Speaker color assignment ─────────────────────────────────────────────────

const SPEAKER_COLORS = [
  "hsl(226 80% 78%)",  // periwinkle (primary)
  "hsl(142 70% 65%)",  // green
  "hsl(38 90% 72%)",   // amber
  "hsl(354 85% 75%)",  // red
  "hsl(280 70% 75%)",  // purple
  "hsl(180 60% 65%)",  // teal
];

function getSpeakerColor(speakerId: string, colorMap: Map<string, string>): string {
  if (colorMap.has(speakerId)) return colorMap.get(speakerId)!;
  const color = SPEAKER_COLORS[colorMap.size % SPEAKER_COLORS.length];
  colorMap.set(speakerId, color);
  return color;
}

function displayNameFromEmail(email?: string | null): string {
  if (!email) return "You";
  return email.split("@")[0] || email;
}

function dedupeTasks(tasks: WsTask[]): WsTask[] {
  const byId = new Map<string, WsTask>();

  for (const task of tasks) {
    byId.set(task.task_id, { ...byId.get(task.task_id), ...task });
  }

  return Array.from(byId.values());
}

function upsertTask(tasks: WsTask[], task: WsTask): WsTask[] {
  return dedupeTasks([...tasks, task]);
}

// ── Streaming text animation ────────────────────────────────────────────────

function StreamingText({ text, animate }: { text: string; animate: boolean }) {
  const [visibleCount, setVisibleCount] = useState(animate ? 0 : text.length);
  const prevTextRef = useRef(text);

  useEffect(() => {
    if (!animate) { setVisibleCount(text.length); return; }
    if (text !== prevTextRef.current) {
      prevTextRef.current = text;
      setVisibleCount(0);
    }
    if (visibleCount >= text.length) return;
    const id = setTimeout(() => setVisibleCount((c) => Math.min(c + 2, text.length)), 8);
    return () => clearTimeout(id);
  }, [text, visibleCount, animate]);

  if (!animate || visibleCount >= text.length) return <>{text}</>;
  return (
    <>
      {text.slice(0, visibleCount)}
      <span className="inline-block w-[2px] h-[13px] bg-foreground/50 ml-px animate-pulse align-text-bottom" />
    </>
  );
}

// ── Props ────────────────────────────────────────────────────────────────────

interface LiveMeetingViewProps {
  meetingId: string;
  onBack: () => void;
}

// ── Component ────────────────────────────────────────────────────────────────

export function LiveMeetingView({ meetingId, onBack }: LiveMeetingViewProps) {
  const [connected, setConnected] = useState(false);
  const [ended, setEnded] = useState(false);
  const [chunks, setChunks] = useState<TranscriptChunk[]>([]);
  const [tasks, setTasks] = useState<WsTask[]>([]);
  const [participants, setParticipants] = useState<WsParticipant[]>([]);
  const [selectedTasks, setSelectedTasks] = useState<Set<string>>(new Set());
  const [pushing, setPushing] = useState(false);
  const [pushResult, setPushResult] = useState<string | null>(null);
  const [muted, setMuted] = useState(false);
  const [sttRunning, setSttRunning] = useState(false);
  const [captureRunning, setCaptureRunning] = useState(false);
  const [meeting, setMeeting] = useState<MeetingDetail | null>(null);
  const [controlBusy, setControlBusy] = useState(false);
  const [ending, setEnding] = useState(false);
  const [controlError, setControlError] = useState<string | null>(null);
  const [reconnecting, setReconnecting] = useState(false);
  const [reconnected, setReconnected] = useState(false);

  const wsRef = useRef<WebSocket | null>(null);
  const pingIntervalRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const transcriptEndRef = useRef<HTMLDivElement>(null);
  const speakerColorMap = useRef<Map<string, string>>(new Map());
  const newestChunkRef = useRef(-1);
  const chunkIndexRef = useRef(0);
  const highestChunkIndexRef = useRef(-1);
  const reconnectTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const backoffRef = useRef(1000); // start at 1s, doubles up to 8s
  const unmountedRef = useRef(false);
  const endedRef = useRef(false); // track ended via ref to avoid stale closures
  const isFirstConnectRef = useRef(true);

  // Keep endedRef in sync with state (avoids stale closures in WS callbacks)
  useEffect(() => {
    endedRef.current = ended;
  }, [ended]);

  // Auto-scroll to bottom when new chunks arrive
  useEffect(() => {
    transcriptEndRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [chunks]);

  // ── Meeting audio pipeline (start on mount, stop on unmount) ────────────
  useEffect(() => {
    let cancelled = false;
    invoke<MeetingSttStatus>("start_meeting_stt")
      .then((status) => {
        if (!cancelled) {
          setSttRunning(status.active);
          setMuted(status.muted);
          setCaptureRunning(status.capture_running);
        }
      })
      .catch((e) => {
        console.warn("[meeting_audio] start failed:", e);
      });

    return () => {
      cancelled = true;
      invoke("stop_meeting_stt").catch(() => {});
      setSttRunning(false);
      setCaptureRunning(false);
    };
  }, []);

  useEffect(() => {
    const unlistenPromise = listen<MeetingSttStatus>("meeting-stt-state", (event) => {
      setSttRunning(event.payload.active);
      setMuted(event.payload.muted);
      setCaptureRunning(event.payload.capture_running);
    });

    invoke<MeetingSttStatus>("get_meeting_stt_status")
      .then((status) => {
        setSttRunning(status.active);
        setMuted(status.muted);
        setCaptureRunning(status.capture_running);
      })
      .catch(() => {});

    return () => {
      unlistenPromise.then((fn) => fn());
    };
  }, []);

  // ── Listen for meeting-transcript events from the native pipeline ───────
  useEffect(() => {
    const conn = getConnection();
    const speakerId = conn?.accountId ?? "self";
    const speakerName = conn?.larkName ?? displayNameFromEmail(conn?.email);

    const unlistenPromise = listen<{ text: string; timestamp_ms: number }>(
      "meeting-transcript",
      (event) => {
        const { text, timestamp_ms } = event.payload;
        if (!text.trim()) return;

        // Add to local transcript as own speech
        const idx = chunkIndexRef.current++;
        const ownChunk: TranscriptChunk = {
          speaker_id: speakerId,
          speaker_name: speakerName,
          text,
          timestamp_ms,
          chunk_index: idx,
        };
        setChunks((prev) => [...prev, ownChunk]);

        // Send to the meeting WS so other participants see it
        const ws = wsRef.current;
        if (ws && ws.readyState === WebSocket.OPEN) {
          ws.send(
            JSON.stringify({
              type: "transcript_chunk",
              text,
              timestamp_ms,
            })
          );
        }
      }
    );

    return () => {
      unlistenPromise.then((fn) => fn());
    };
  }, []);

  // ── Toggle mute handler ─────────────────────────────────────────────────
  const handleToggleMute = useCallback(async () => {
    setControlBusy(true);
    setControlError(null);
    try {
      const status = await invoke<MeetingSttStatus>("toggle_meeting_mute");
      setMuted(status.muted);
      setSttRunning(status.active);
      setCaptureRunning(status.capture_running);
    } catch (e) {
      console.warn("[meeting_audio] toggle mute failed:", e);
      setControlError(e instanceof Error ? e.message : String(e));
    } finally {
      setControlBusy(false);
    }
  }, []);

  const handleLeave = useCallback(async () => {
    setControlBusy(true);
    try {
      await invoke("stop_meeting_stt");
    } catch {}
    setControlBusy(false);
    onBack();
  }, [onBack]);

  const handleEndMeeting = useCallback(async () => {
    const conn = getConnection();
    if (!conn || ending) return;

    setEnding(true);
    setControlError(null);
    try {
      const url = conn.serverUrl.replace(/\/+$/, "");
      const res = await fetch(`${url}/v1/meetings/${meetingId}/end`, {
        method: "POST",
        headers: { Authorization: `Bearer ${conn.jwt}` },
      });
      if (!res.ok) {
        const data = await res.json().catch(() => ({}));
        throw new Error(data.error ?? "Failed to end meeting");
      }
      await invoke("stop_meeting_stt").catch(() => {});
      setEnded(true);
    } catch (e) {
      setControlError(e instanceof Error ? e.message : String(e));
    } finally {
      setEnding(false);
    }
  }, [ending, meetingId]);

  useEffect(() => {
    const conn = getConnection();
    if (!conn) return;

    let cancelled = false;
    const url = conn.serverUrl.replace(/\/+$/, "");
    fetch(`${url}/v1/meetings/${meetingId}`, {
      headers: { Authorization: `Bearer ${conn.jwt}` },
    })
      .then((res) => (res.ok ? res.json() : null))
      .then((data) => {
        if (cancelled || !data?.meeting) return;
        setMeeting(data.meeting);
        if (data.meeting.status === "ended") {
          setEnded(true);
          invoke("stop_meeting_stt").catch(() => {});
        }
      })
      .catch(() => {});

    return () => {
      cancelled = true;
    };
  }, [meetingId]);

  // WebSocket connection with auto-reconnect and resume protocol
  useEffect(() => {
    const connOrNull = getConnection();
    if (!connOrNull) return;

    // Capture non-null for use in nested closures (TS narrowing doesn't propagate)
    const conn = connOrNull;

    unmountedRef.current = false;
    isFirstConnectRef.current = true;

    const wsUrl = conn.serverUrl
      .replace(/^http:\/\//, "ws://")
      .replace(/^https:\/\//, "wss://")
      .replace(/\/+$/, "");

    function clearPing() {
      if (pingIntervalRef.current) {
        clearInterval(pingIntervalRef.current);
        pingIntervalRef.current = null;
      }
    }

    function clearReconnectTimer() {
      if (reconnectTimerRef.current) {
        clearTimeout(reconnectTimerRef.current);
        reconnectTimerRef.current = null;
      }
    }

    function scheduleReconnect() {
      // Don't reconnect if unmounted or meeting ended
      if (unmountedRef.current || endedRef.current) return;

      setReconnecting(true);
      const delay = backoffRef.current;
      reconnectTimerRef.current = setTimeout(() => {
        reconnectTimerRef.current = null;
        if (!unmountedRef.current && !endedRef.current) {
          // Double backoff for next attempt, cap at 8s
          backoffRef.current = Math.min(backoffRef.current * 2, 8000);
          connectWs();
        }
      }, delay);
    }

    function connectWs() {
      // Don't connect if unmounted or meeting ended
      if (unmountedRef.current || endedRef.current) return;

      const ws = new WebSocket(`${wsUrl}/v1/meetings/${meetingId}/ws?token=${conn.jwt}`);
      wsRef.current = ws;

      ws.onopen = () => {
        setConnected(true);
        setReconnecting(false);

        // Reset backoff on successful connection
        backoffRef.current = 1000;

        const isReconnect = !isFirstConnectRef.current;
        isFirstConnectRef.current = false;

        // On reconnect, send resume with last known chunk index
        if (isReconnect && highestChunkIndexRef.current >= 0) {
          ws.send(JSON.stringify({
            type: "resume",
            last_chunk_index: highestChunkIndexRef.current,
          }));

          // Flash "Reconnected" briefly
          setReconnected(true);
          setTimeout(() => setReconnected(false), 2000);
        }

        // Start keep-alive ping every 25s
        clearPing();
        pingIntervalRef.current = setInterval(() => {
          if (ws.readyState === WebSocket.OPEN) {
            ws.send(JSON.stringify({ type: "ping" }));
          }
        }, 25_000);
      };

      ws.onclose = () => {
        setConnected(false);
        clearPing();
        // Only close the ref if it's still pointing to this socket
        if (wsRef.current === ws) {
          wsRef.current = null;
        }
        scheduleReconnect();
      };

      ws.onerror = () => {
        // onerror is always followed by onclose, so reconnect happens there
        setConnected(false);
      };

      ws.onmessage = (event) => {
        try {
          const msg: WsMessage = JSON.parse(event.data);

          switch (msg.type) {
            case "connected":
              // Successfully authenticated
              break;

            case "catchup":
              // On catchup, merge chunks and update highest index
              for (const c of msg.current_chunks) {
                if (c.chunk_index > highestChunkIndexRef.current) {
                  highestChunkIndexRef.current = c.chunk_index;
                }
              }
              setChunks((prev) => {
                // Build a set of existing chunk_index values for dedup
                const existingIndices = new Set(prev.map((c) => c.chunk_index));
                const newChunks = msg.current_chunks.filter(
                  (c) => !existingIndices.has(c.chunk_index)
                );
                if (newChunks.length === 0) return prev;
                // Merge and sort by chunk_index to maintain order
                const merged = [...prev, ...newChunks];
                merged.sort((a, b) => a.chunk_index - b.chunk_index);
                return merged;
              });
              setTasks(dedupeTasks(msg.tasks));
              setParticipants(msg.participants);
              break;

            case "transcript_chunk": {
              const newChunk: TranscriptChunk = {
                speaker_id: msg.speaker_id,
                speaker_name: msg.speaker_name,
                text: msg.text,
                timestamp_ms: msg.timestamp_ms,
                chunk_index: msg.chunk_index,
              };
              // Track highest chunk_index
              if (msg.chunk_index > highestChunkIndexRef.current) {
                highestChunkIndexRef.current = msg.chunk_index;
              }
              // Deduplicate by chunk_index
              setChunks((prev) => {
                if (prev.some((c) => c.chunk_index === msg.chunk_index)) {
                  return prev; // already have this chunk
                }
                return [...prev, newChunk];
              });
              break;
            }

            case "task_detected":
              setTasks((prev) => upsertTask(prev, {
                task_id: msg.task_id,
                title: msg.title,
                assignee_name: msg.assignee_name,
              }));
              break;

            case "summary_updated":
              // Could show summary somewhere; for now just acknowledge
              break;

            case "participant_joined":
              setParticipants((prev) => [
                ...prev.filter((p) => p.account_id !== msg.account_id),
                {
                  account_id: msg.account_id,
                  email: msg.email,
                  display_name: msg.display_name,
                  status: "active",
                },
              ]);
              break;

            case "participant_left":
              setParticipants((prev) =>
                prev.map((p) =>
                  p.account_id === msg.account_id ? { ...p, status: "left" } : p
                )
              );
              break;

            case "meeting_ended":
              setEnded(true);
              invoke("stop_meeting_stt").catch(() => {});
              break;
          }
        } catch {
          // Ignore malformed messages
        }
      };
    }

    // Initial connection
    connectWs();

    return () => {
      unmountedRef.current = true;
      clearPing();
      clearReconnectTimer();
      setReconnecting(false);
      if (wsRef.current) {
        wsRef.current.close();
        wsRef.current = null;
      }
    };
  }, [meetingId]);

  // Toggle task selection
  const toggleTask = useCallback((taskId: string) => {
    setSelectedTasks((prev) => {
      const next = new Set(prev);
      if (next.has(taskId)) next.delete(taskId);
      else next.add(taskId);
      return next;
    });
  }, []);

  // Push selected tasks to Lark
  const handlePushTasks = useCallback(async () => {
    const conn = getConnection();
    if (!conn || selectedTasks.size === 0) return;

    setPushing(true);
    setPushResult(null);

    try {
      const url = conn.serverUrl.replace(/\/+$/, "");
      const res = await fetch(`${url}/v1/meetings/${meetingId}/push-tasks`, {
        method: "POST",
        headers: {
          Authorization: `Bearer ${conn.jwt}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify({ task_ids: Array.from(selectedTasks) }),
      });

      if (!res.ok) {
        setPushResult("Failed to push tasks");
        return;
      }

      const data = await res.json();
      setPushResult(`Pushed ${data.pushed} task${data.pushed !== 1 ? "s" : ""}`);

      // Mark pushed tasks as synced locally
      setTasks((prev) =>
        prev.map((t) =>
          selectedTasks.has(t.task_id)
            ? { ...t, status: "synced", lark_task_id: "pushed" }
            : t
        )
      );
      setSelectedTasks(new Set());

      // Clear result message after 3s
      setTimeout(() => setPushResult(null), 3000);
    } catch {
      setPushResult("Network error");
    } finally {
      setPushing(false);
    }
  }, [meetingId, selectedTasks]);

  // Check if a task is already synced
  const isTaskSynced = (task: WsTask) =>
    task.status === "synced" || !!task.lark_task_id;

  // Count selectable (non-synced) selected tasks
  const selectableSelected = Array.from(selectedTasks).filter(
    (id) => !isTaskSynced(tasks.find((t) => t.task_id === id)!)
  ).length;

  // Format timestamp
  const formatTime = (ms: number) => {
    const totalSec = Math.floor(ms / 1000);
    const min = Math.floor(totalSec / 60);
    const sec = totalSec % 60;
    return `${min}:${sec.toString().padStart(2, "0")}`;
  };

  // Active participant count
  const activeParticipants = participants.filter((p) => p.status !== "left").length;
  const conn = getConnection();
  const isOwner = !!meeting && !!conn && meeting.created_by === conn.accountId;
  const captureLabel = captureRunning ? "Recording" : muted ? "Muted" : "Paused";

  // ── Ended overlay ──────────────────────────────────────────────────────────

  if (ended) {
    return (
      <div className="h-full flex flex-col items-center justify-center gap-4">
        <div
          className="flex items-center justify-center w-14 h-14 rounded-full"
          style={{ background: "hsl(var(--surface-4))" }}
        >
          <Radio size={24} style={{ color: "hsl(var(--muted-foreground))" }} />
        </div>
        <h2 className="text-[16px] font-bold text-foreground">Meeting Ended</h2>
        <p className="text-[12px] text-muted-foreground">
          This meeting has concluded. {chunks.length} transcript chunk{chunks.length !== 1 ? "s" : ""} recorded.
        </p>
        <button
          onClick={onBack}
          className="flex items-center gap-2 px-4 py-2 rounded-lg text-[12px] font-semibold transition-colors mt-2"
          style={{
            background: "hsl(var(--primary))",
            color: "hsl(var(--primary-foreground))",
          }}
        >
          <ArrowLeft size={13} />
          Back to Meetings
        </button>
      </div>
    );
  }

  // ── Main layout ────────────────────────────────────────────────────────────

  return (
    <div className="h-full flex flex-col overflow-hidden relative">
      {/* Top bar */}
      <div
        className="flex items-center justify-between px-5 py-3 flex-shrink-0"
        style={{
          borderBottom: "1px solid hsl(var(--surface-4))",
        }}
      >
        <div className="flex items-center gap-3">
          <button
            onClick={() => void handleLeave()}
            className="flex items-center justify-center w-7 h-7 rounded-lg transition-colors hover:opacity-80"
            style={{ background: "hsl(var(--surface-4))" }}
            title="Leave meeting"
          >
            <ArrowLeft size={14} style={{ color: "hsl(var(--foreground))" }} />
          </button>
          <div className="flex items-center gap-2">
            <Radio size={14} style={{ color: "hsl(var(--primary))" }} />
            <h1 className="text-[14px] font-bold text-foreground">Live Meeting</h1>
          </div>
        </div>

        <div className="flex items-center gap-4">
          {/* Participant count */}
          <div className="flex items-center gap-1.5 text-[11px] text-muted-foreground">
            <Users size={12} />
            <span>{activeParticipants}</span>
          </div>

          {/* Connection status */}
          <div className="flex items-center gap-1.5">
            {reconnecting ? (
              <>
                <Loader2 size={12} className="animate-spin" style={{ color: "hsl(38 90% 72%)" }} />
                <span className="text-[10px] font-medium" style={{ color: "hsl(38 90% 72%)" }}>
                  Reconnecting...
                </span>
              </>
            ) : reconnected ? (
              <>
                <span
                  className="w-2 h-2 rounded-full"
                  style={{
                    background: "hsl(142 70% 55%)",
                    boxShadow: "0 0 6px hsl(142 70% 55% / 0.5)",
                  }}
                />
                <span className="text-[10px] font-medium" style={{ color: "hsl(142 70% 65%)" }}>
                  Reconnected
                </span>
              </>
            ) : connected ? (
              <>
                <span
                  className="w-2 h-2 rounded-full"
                  style={{
                    background: "hsl(142 70% 55%)",
                    boxShadow: "0 0 6px hsl(142 70% 55% / 0.5)",
                  }}
                />
                <Wifi size={12} style={{ color: "hsl(142 70% 65%)" }} />
              </>
            ) : (
              <>
                <span
                  className="w-2 h-2 rounded-full"
                  style={{ background: "hsl(354 80% 62%)" }}
                />
                <WifiOff size={12} style={{ color: "hsl(354 85% 75%)" }} />
              </>
            )}
          </div>

          <button
            onClick={() => void handleLeave()}
            disabled={controlBusy}
            className="flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg text-[11px] font-semibold transition-colors disabled:opacity-40"
            style={{
              background: "hsl(var(--surface-4))",
              color: "hsl(var(--foreground))",
            }}
            title="Leave meeting"
          >
            <PhoneOff size={12} />
            Leave
          </button>

          {isOwner && (
            <button
              onClick={() => void handleEndMeeting()}
              disabled={ending}
              className="flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg text-[11px] font-semibold transition-colors disabled:opacity-40"
              style={{
                background: "hsl(354 80% 55% / 0.14)",
                color: "hsl(354 85% 75%)",
              }}
              title="End meeting for everyone"
            >
              {ending ? <Loader2 size={12} className="animate-spin" /> : <CircleStop size={12} />}
              End
            </button>
          )}
        </div>
      </div>

      {/* Main content: left transcript + right tasks */}
      <div className="flex flex-1 overflow-hidden min-h-0">
        {/* Left panel — Transcript (60%) */}
        <div
          className="flex-[3] flex flex-col overflow-hidden min-w-0"
          style={{ borderRight: "1px solid hsl(var(--surface-4))" }}
        >
          <div className="px-5 py-3 flex-shrink-0">
            <h2 className="text-[12px] font-semibold text-muted-foreground uppercase tracking-wide">
              Transcript
            </h2>
          </div>
          <div className="flex-1 overflow-y-auto px-5 pb-24">
            {chunks.length === 0 ? (
              <div className="flex flex-col items-center justify-center h-full gap-2 opacity-50">
                <Loader2 size={18} className="animate-spin text-muted-foreground" />
                <p className="text-[11px] text-muted-foreground">
                  Waiting for transcript...
                </p>
              </div>
            ) : (
              <div className="space-y-3">
                {chunks.map((chunk, i) => {
                  const color = getSpeakerColor(chunk.speaker_id, speakerColorMap.current);
                  const prevChunk = i > 0 ? chunks[i - 1] : null;
                  const sameSpeaker = prevChunk?.speaker_id === chunk.speaker_id;
                  const isNewest = i === chunks.length - 1 && chunk.chunk_index > newestChunkRef.current;
                  if (isNewest) newestChunkRef.current = chunk.chunk_index;

                  return (
                    <div key={`${chunk.chunk_index}-${i}`} className={sameSpeaker ? "mt-1" : ""}>
                      {!sameSpeaker && (
                        <div className="flex items-center gap-2 mb-1">
                          <span
                            className="text-[11px] font-bold"
                            style={{ color }}
                          >
                            {chunk.speaker_name}
                          </span>
                          <span className="text-[10px] text-muted-foreground">
                            {formatTime(chunk.timestamp_ms)}
                          </span>
                        </div>
                      )}
                      <p className="text-[12px] text-foreground leading-relaxed pl-0">
                        <StreamingText text={chunk.text} animate={isNewest} />
                      </p>
                    </div>
                  );
                })}
                <div ref={transcriptEndRef} />
              </div>
            )}
          </div>
        </div>

        {/* Right panel — Tasks (40%) */}
        <div className="flex-[2] flex flex-col overflow-hidden min-w-0">
          <div className="px-4 py-3 flex-shrink-0">
            <h2 className="text-[12px] font-semibold text-muted-foreground uppercase tracking-wide">
              Tasks ({tasks.length})
            </h2>
          </div>

          <div className="flex-1 overflow-y-auto px-4 pb-24">
            {tasks.length === 0 ? (
              <div className="flex flex-col items-center justify-center h-full gap-2 opacity-50">
                <Check size={18} className="text-muted-foreground" />
                <p className="text-[11px] text-muted-foreground">
                  Tasks will appear here
                </p>
              </div>
            ) : (
              <div className="space-y-2">
                {tasks.map((task) => {
                  const synced = isTaskSynced(task);
                  const isSelected = selectedTasks.has(task.task_id);

                  return (
                    <div
                      key={task.task_id}
                      onClick={() => !synced && toggleTask(task.task_id)}
                      className={`rounded-lg p-3 transition-colors ${
                        synced ? "opacity-50 cursor-default" : "cursor-pointer"
                      }`}
                      style={{
                        background: isSelected && !synced
                          ? "hsl(var(--primary) / 0.08)"
                          : "hsl(var(--surface-3))",
                        boxShadow: isSelected && !synced
                          ? "inset 0 0 0 1px hsl(var(--primary) / 0.3)"
                          : "inset 0 0 0 1px hsl(var(--surface-4))",
                      }}
                    >
                      <div className="flex items-start gap-2.5">
                        {/* Checkbox */}
                        <div
                          className="flex items-center justify-center w-4 h-4 rounded flex-shrink-0 mt-0.5 transition-colors"
                          style={{
                            background: synced
                              ? "hsl(142 70% 45% / 0.2)"
                              : isSelected
                                ? "hsl(var(--primary))"
                                : "hsl(var(--surface-4))",
                            border: synced
                              ? "none"
                              : isSelected
                                ? "none"
                                : "1px solid hsl(var(--muted-foreground) / 0.3)",
                          }}
                        >
                          {(synced || isSelected) && (
                            <Check
                              size={10}
                              style={{
                                color: synced
                                  ? "hsl(142 70% 65%)"
                                  : "hsl(var(--primary-foreground))",
                              }}
                            />
                          )}
                        </div>

                        <div className="flex-1 min-w-0">
                          <p className="text-[12px] text-foreground leading-snug">
                            {task.title}
                          </p>
                          {task.assignee_name && (
                            <p className="text-[10px] text-muted-foreground mt-1">
                              {task.assignee_name}
                            </p>
                          )}
                          {synced && (
                            <p className="text-[10px] mt-1" style={{ color: "hsl(142 70% 65%)" }}>
                              Synced to Lark
                            </p>
                          )}
                        </div>
                      </div>
                    </div>
                  );
                })}
              </div>
            )}
          </div>

          {/* Push to Lark button */}
          <div
            className="flex-shrink-0 px-4 py-3"
            style={{ borderTop: "1px solid hsl(var(--surface-4))" }}
          >
            {pushResult && (
              <p className="text-[11px] text-center mb-2" style={{ color: "hsl(142 70% 65%)" }}>
                {pushResult}
              </p>
            )}
            <button
              onClick={() => void handlePushTasks()}
              disabled={selectableSelected === 0 || pushing}
              className="w-full flex items-center justify-center gap-2 px-4 py-2 rounded-lg text-[12px] font-semibold transition-all disabled:opacity-40 disabled:cursor-not-allowed"
              style={{
                background: selectableSelected > 0
                  ? "hsl(var(--primary))"
                  : "hsl(var(--surface-4))",
                color: selectableSelected > 0
                  ? "hsl(var(--primary-foreground))"
                  : "hsl(var(--muted-foreground))",
              }}
            >
              {pushing ? (
                <>
                  <Loader2 size={13} className="animate-spin" />
                  Pushing...
                </>
              ) : (
                <>
                  <Send size={12} />
                  Push {selectableSelected > 0 ? `${selectableSelected} ` : ""}to Lark
                </>
              )}
            </button>
          </div>
        </div>
      </div>

      {/* Meeting control dock */}
      <div className="absolute left-6 bottom-4 z-20">
        <div
          className="flex items-center gap-2 px-2.5 py-2 rounded-full"
          style={{
            background: "hsl(var(--surface-2) / 0.94)",
            border: "1px solid hsl(var(--surface-4))",
            boxShadow: "0 18px 42px hsl(0 0% 0% / 0.38)",
            backdropFilter: "blur(22px) saturate(160%)",
            WebkitBackdropFilter: "blur(22px) saturate(160%)",
          }}
        >
          <button
            onClick={() => void handleLeave()}
            disabled={controlBusy}
            className="flex items-center justify-center w-10 h-10 rounded-full transition-all disabled:opacity-40"
            style={{
              background: "hsl(var(--surface-4))",
              color: "hsl(var(--foreground))",
            }}
            title="Leave meeting"
          >
            <PhoneOff size={16} />
          </button>

          <button
            onClick={() => void handleToggleMute()}
            disabled={!sttRunning || controlBusy}
            className="flex items-center gap-2 h-10 px-4 rounded-full text-[12px] font-semibold transition-all disabled:opacity-40"
            style={{
              background: captureRunning
                ? "hsl(142 70% 45% / 0.18)"
                : "hsl(354 80% 55% / 0.16)",
              color: captureRunning
                ? "hsl(142 70% 65%)"
                : "hsl(354 85% 75%)",
              boxShadow: captureRunning
                ? "0 0 18px hsl(142 70% 55% / 0.24)"
                : "none",
            }}
            title={captureRunning ? "Mute meeting capture" : "Resume meeting capture"}
          >
            {controlBusy ? (
              <Loader2 size={15} className="animate-spin" />
            ) : captureRunning ? (
              <Mic size={15} />
            ) : (
              <MicOff size={15} />
            )}
            <span>{captureLabel}</span>
            {captureRunning && (
              <span
                className="w-2 h-2 rounded-full animate-pulse"
                style={{
                  background: "hsl(142 70% 55%)",
                  boxShadow: "0 0 7px hsl(142 70% 55% / 0.7)",
                }}
              />
            )}
          </button>

          <div
            className="h-8 w-px"
            style={{ background: "hsl(var(--surface-4))" }}
          />

          <div className="flex items-center gap-2 px-2 text-[11px] text-muted-foreground">
            <Users size={13} />
            <span>{activeParticipants}</span>
            {reconnecting ? (
              <Loader2 size={13} className="animate-spin" style={{ color: "hsl(38 90% 72%)" }} />
            ) : connected ? (
              <Wifi size={13} style={{ color: "hsl(142 70% 65%)" }} />
            ) : (
              <WifiOff size={13} style={{ color: "hsl(354 85% 75%)" }} />
            )}
          </div>

          {isOwner && (
            <button
              onClick={() => void handleEndMeeting()}
              disabled={ending}
              className="flex items-center justify-center w-10 h-10 rounded-full transition-all disabled:opacity-40"
              style={{
                background: "hsl(354 80% 55% / 0.16)",
                color: "hsl(354 85% 75%)",
              }}
              title="End meeting for everyone"
            >
              {ending ? <Loader2 size={15} className="animate-spin" /> : <CircleStop size={16} />}
            </button>
          )}
        </div>
        {controlError && (
          <div
            className="mt-2 px-3 py-1.5 rounded-full text-[11px] text-center"
            style={{
              background: "hsl(354 80% 55% / 0.15)",
              color: "hsl(354 85% 75%)",
              border: "1px solid hsl(354 80% 55% / 0.18)",
            }}
          >
            {controlError}
          </div>
        )}
      </div>
    </div>
  );
}
