import { useEffect, useState, useRef, useCallback, useMemo } from "react";
import {
  ArrowLeft,
  Radio,
  Users,
  Loader2,
  Wifi,
  WifiOff,
  Mic,
  MicOff,
  PhoneOff,
  CircleStop,
  Sparkles,
  X,
} from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getConnection } from "@/lib/enterprise";
import { MeetingAiChat } from "@/components/MeetingAiChat";
import { formatTimestamp, speakerColor } from "@/lib/meetingFormat";

// ── Types (matching WS protocol) ─────────────────────────────────────────────

interface TranscriptChunk {
  speaker_id: string;
  speaker_name: string;
  text: string;
  timestamp_ms: number;
  chunk_index: number;
  source?: string;
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
  scheduled_at?: string | null;
  created_at?: string | null;
  agenda?: string | null;
}


interface MeetingEngineStatus {
  active: boolean;
  muted: boolean;
  capture_running: boolean;
  mic_track_active?: boolean;
  system_track_active?: boolean;
  speaker_reference_available?: boolean;
  echo_gate_active?: boolean;
  local_speech_active?: boolean;
  last_gate_reason?: string;
  session_id?: string | null;
  started_at_ms?: number | null;
  generation?: number;
  phase?: string;
  mic_wav_path?: string | null;
  mic_duration_ms?: number | null;
  mic_samples_written?: number;
  mic_dropped_chunks?: number;
  system_wav_path?: string | null;
  system_duration_ms?: number | null;
  system_samples_written?: number;
  system_dropped_chunks?: number;
  system_capture_status?: string;
  system_capture_error?: string | null;
  merged_wav_path?: string | null;
  merged_duration_ms?: number | null;
  merge_status?: string;
  merge_error?: string | null;
  source_activity_path?: string | null;
  live_transcript_running?: boolean;
  live_transcript_status?: string;
  live_transcript_provider?: string | null;
  live_transcript_model?: string | null;
  live_transcript_language?: string | null;
  live_transcript_chunk_count?: number;
  live_transcript_error?: string | null;
  live_transcript_dropped_audio_chunks?: number;
  transcription_running?: boolean;
  transcription_status?: string;
  transcription_provider?: string | null;
  transcription_model?: string | null;
  transcription_language?: string | null;
  transcription_latency_ms?: number | null;
  transcript_text_path?: string | null;
  transcript_json_path?: string | null;
  transcript_text?: string | null;
  transcript_cleaned_text?: string | null;
  final_transcript_text?: string | null;
  transcript_cleanup_status?: string;
  transcript_cleanup_provider?: string | null;
  transcript_cleanup_model?: string | null;
  transcript_cleanup_latency_ms?: number | null;
  transcript_cleanup_error?: string | null;
  final_diarization_status?: string;
  final_diarization_provider?: string | null;
  final_diarization_latency_ms?: number | null;
  final_diarization_json_path?: string | null;
  final_transcript_json_path?: string | null;
  final_diarization_error?: string | null;
  transcription_error?: string | null;
  last_error?: string | null;
}

type TranscriptReviewMode = "final" | "cleaned" | "raw";

interface MeetingLiveTranscriptChunk {
  chunk_index: number;
  source: string;
  speaker_id: string;
  speaker_name: string;
  timestamp_ms: number;
  text: string;
  is_final: boolean;
}

interface MeetingLiveTranscriptPayload {
  session_id?: string | null;
  status: string;
  provider?: string | null;
  model?: string | null;
  language?: string | null;
  chunks: MeetingLiveTranscriptChunk[];
  error?: string | null;
  dropped_audio_chunks?: number;
}

interface MeetingLiveTranscriptEvent {
  session_id: string;
  chunk: MeetingLiveTranscriptChunk;
}

interface MeetingAiActionItem {
  title: string;
  assignee?: string | null;
  due?: string | null;
  evidence?: string | null;
}

interface MeetingAiDecision {
  text: string;
  evidence?: string | null;
}

interface MeetingIntelligenceResult {
  status: string;
  provider: string;
  model: string;
  latency_ms: number;
  transcript_source: string;
  summary: string;
  action_items: MeetingAiActionItem[];
  decisions: MeetingAiDecision[];
}


type WsMessage =
  | { type: "connected"; meeting_id: string; account_id: string }
  | { type: "catchup"; summary: string | null; current_chunks: TranscriptChunk[]; decisions: { text: string }[]; participants: WsParticipant[] }
  | { type: "transcript_chunk"; speaker_id: string; speaker_name: string; text: string; timestamp_ms: number; chunk_index: number }
  | { type: "summary_updated"; summary: string }
  | { type: "participant_joined"; account_id: string; email: string; display_name?: string }
  | { type: "participant_left"; account_id: string }
  | { type: "meeting_ended"; meeting_id: string };

function liveChunkToTranscriptChunk(chunk: MeetingLiveTranscriptChunk): TranscriptChunk {
  return {
    speaker_id: chunk.speaker_id,
    speaker_name: chunk.speaker_name,
    text: chunk.text,
    timestamp_ms: chunk.timestamp_ms,
    chunk_index: 1_000_000_000 + chunk.chunk_index,
    source: chunk.source,
  };
}

function transcriptChunkKey(chunk: TranscriptChunk): string {
  return [
    chunk.source || "remote",
    chunk.speaker_id,
    Math.round(chunk.timestamp_ms),
    chunk.text.trim(),
  ].join("|");
}

function mergeTranscriptChunks(
  current: TranscriptChunk[],
  incoming: TranscriptChunk[],
): TranscriptChunk[] {
  if (incoming.length === 0) return current;
  const seen = new Set(current.map(transcriptChunkKey));
  let changed = false;
  const merged = [...current];
  for (const chunk of incoming) {
    const key = transcriptChunkKey(chunk);
    if (seen.has(key)) continue;
    seen.add(key);
    merged.push(chunk);
    changed = true;
  }
  if (!changed) return current;
  merged.sort((a, b) =>
    a.timestamp_ms - b.timestamp_ms
    || a.chunk_index - b.chunk_index
    || a.speaker_id.localeCompare(b.speaker_id)
  );
  return merged;
}

// ── Props ────────────────────────────────────────────────────────────────────

interface LiveMeetingViewProps {
  meetingId: string;
  onBack: () => void;
  /** Fired once when the meeting ends. The parent navigates to the Meetings
   *  page and focuses this meeting; LiveMeetingView is the LIVE surface only and
   *  no longer renders its own post-meeting notes layout. */
  onEnded?: (meetingId: string) => void;
}

// ── Component ────────────────────────────────────────────────────────────────

export function LiveMeetingView({ meetingId, onBack, onEnded }: LiveMeetingViewProps) {
  void onBack; // onBack reserved for a future in-call back button
  const [connected, setConnected] = useState(false);
  const [ended, setEnded] = useState(false);
  const [chunks, setChunks] = useState<TranscriptChunk[]>([]);
  const [participants, setParticipants] = useState<WsParticipant[]>([]);
  // Live notes — saved to this meeting and used as AI-chat context.
  const [notes, setNotes] = useState("");
  const [notesStatus, setNotesStatus] = useState<"idle" | "saving" | "saved">("idle");
  const notesSaveTimer = useRef<number | null>(null);
  const [muted, setMuted] = useState(false);
  const [engineRunning, setEngineRunning] = useState(false);
  const [captureRunning, setCaptureRunning] = useState(false);
  const [micTrackActive, setMicTrackActive] = useState(false);
  const [systemTrackActive, setSystemTrackActive] = useState(false);
  const [systemCaptureStatus, setSystemCaptureStatus] = useState("idle");
  const [systemCaptureError, setSystemCaptureError] = useState<string | null>(null);
  const [localSpeechActive, setLocalSpeechActive] = useState(false);
  const [lastGateReason, setLastGateReason] = useState("not_started");
  const [engineError, setEngineError] = useState<string | null>(null);
  const [liveTranscriptStatus, setLiveTranscriptStatus] = useState("idle");
  const [liveTranscriptModel, setLiveTranscriptModel] = useState<string | null>(null);
  const [liveTranscriptError, setLiveTranscriptError] = useState<string | null>(null);
  const [transcriptionRunning, setTranscriptionRunning] = useState(false);
  const [transcriptionStatus, setTranscriptionStatus] = useState("idle");
  const [, setTranscriptionModel] = useState<string | null>(null);
  const [, setTranscriptionLanguage] = useState<string | null>(null);
  const [, setTranscriptionLatencyMs] = useState<number | null>(null);
  const [, setTranscriptTextPath] = useState<string | null>(null);
  const [, setTranscriptJsonPath] = useState<string | null>(null);
  const [transcriptText, setTranscriptText] = useState<string | null>(null);
  const [transcriptCleanedText, setTranscriptCleanedText] = useState<string | null>(null);
  const [finalTranscriptText, setFinalTranscriptText] = useState<string | null>(null);
  const [finalTranscriptJsonPath, setFinalTranscriptJsonPath] = useState<string | null>(null);
  const [transcriptCleanupStatus, setTranscriptCleanupStatus] = useState("idle");
  const [, setTranscriptCleanupModel] = useState<string | null>(null);
  const [, setTranscriptCleanupLatencyMs] = useState<number | null>(null);
  const [, setTranscriptCleanupError] = useState<string | null>(null);
  const [finalDiarizationStatus, setFinalDiarizationStatus] = useState("idle");
  const [, setFinalDiarizationProvider] = useState<string | null>(null);
  const [, setFinalDiarizationLatencyMs] = useState<number | null>(null);
  const [, setFinalDiarizationError] = useState<string | null>(null);
  const [transcriptionError, setTranscriptionError] = useState<string | null>(null);
  const [transcriptReviewMode, setTranscriptReviewMode] = useState<TranscriptReviewMode>("raw");
  const [meetingAiStatus, setMeetingAiStatus] = useState("idle");
  const [meetingAiSummary, setMeetingAiSummary] = useState("");
  const [, setMeetingAiActionItems] = useState<MeetingAiActionItem[]>([]);
  const [, setMeetingAiDecisions] = useState<MeetingAiDecision[]>([]);
  const [, setMeetingAiSource] = useState<string | null>(null);
  const [, setMeetingAiModel] = useState<string | null>(null);
  const [, setMeetingAiLatencyMs] = useState<number | null>(null);
  const [, setMeetingAiError] = useState<string | null>(null);
  const [aiChatOpen, setAiChatOpen] = useState(false);
  const [meeting, setMeeting] = useState<MeetingDetail | null>(null);
  const [controlBusy, setControlBusy] = useState(false);
  const [ending, setEnding] = useState(false);
  const [controlError, setControlError] = useState<string | null>(null);
  const [reconnecting, setReconnecting] = useState(false);
  const [reconnected, setReconnected] = useState(false);

  const wsRef = useRef<WebSocket | null>(null);
  const pingIntervalRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const transcriptEndRef = useRef<HTMLDivElement>(null);
  const highestChunkIndexRef = useRef(-1);
  const reconnectTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const backoffRef = useRef(1000); // start at 1s, doubles up to 8s
  const unmountedRef = useRef(false);
  const endedRef = useRef(false); // track ended via ref to avoid stale closures
  const isFirstConnectRef = useRef(true);
  const lastIntelligenceKeyRef = useRef<string | null>(null);
  const engineSessionIdRef = useRef<string | null>(null);

  const applyMeetingStatus = useCallback((status: MeetingEngineStatus) => {
    engineSessionIdRef.current = status.session_id || null;
    setEngineRunning(status.active);
    setMuted(status.muted);
    setCaptureRunning(status.capture_running);
    setMicTrackActive(Boolean(status.mic_track_active));
    setSystemTrackActive(Boolean(status.system_track_active));
    setSystemCaptureStatus(status.system_capture_status || "idle");
    setSystemCaptureError(status.system_capture_error || null);
    setLocalSpeechActive(Boolean(status.local_speech_active));
    setLastGateReason(status.last_gate_reason || "unknown");
    setEngineError(status.last_error || null);
    setLiveTranscriptStatus(status.live_transcript_status || "idle");
    setLiveTranscriptModel(status.live_transcript_model || null);
    setLiveTranscriptError(status.live_transcript_error || null);
    setTranscriptionRunning(Boolean(status.transcription_running));
    setTranscriptionStatus(status.transcription_status || "idle");
    setTranscriptionModel(status.transcription_model || null);
    setTranscriptionLanguage(status.transcription_language || null);
    setTranscriptionLatencyMs(status.transcription_latency_ms ?? null);
    setTranscriptTextPath(status.transcript_text_path || null);
    setTranscriptJsonPath(status.transcript_json_path || null);
    setTranscriptText(status.transcript_text || null);
    setTranscriptCleanedText(status.transcript_cleaned_text || null);
    setFinalTranscriptText(status.final_transcript_text || null);
    setFinalTranscriptJsonPath(status.final_transcript_json_path || null);
    setTranscriptCleanupStatus(status.transcript_cleanup_status || "idle");
    setTranscriptCleanupModel(status.transcript_cleanup_model || null);
    setTranscriptCleanupLatencyMs(status.transcript_cleanup_latency_ms ?? null);
    setTranscriptCleanupError(status.transcript_cleanup_error || null);
    setFinalDiarizationStatus(status.final_diarization_status || "idle");
    setFinalDiarizationProvider(status.final_diarization_provider || null);
    setFinalDiarizationLatencyMs(status.final_diarization_latency_ms ?? null);
    setFinalDiarizationError(status.final_diarization_error || null);
    setTranscriptionError(status.transcription_error || null);
  }, []);

  // Keep endedRef in sync with state (avoids stale closures in WS callbacks)
  useEffect(() => {
    endedRef.current = ended;
  }, [ended]);

  // When the meeting ends (via any path — leave, end, WS, or reopening an
  // already-ended meeting) hand off to the Meetings page exactly once. The
  // parent unmounts this view, so the post-meeting experience lives entirely in
  // MeetingsView (single surface; no duplicate notes layout here).
  const onEndedRef = useRef(onEnded);
  useEffect(() => {
    onEndedRef.current = onEnded;
  }, [onEnded]);
  const endedHandoffRef = useRef(false);
  useEffect(() => {
    if (ended && !endedHandoffRef.current) {
      endedHandoffRef.current = true;
      onEndedRef.current?.(meetingId);
    }
  }, [ended, meetingId]);

  // While a meeting is live, show a floating always-on-top pill whenever the app
  // is not in the foreground — switched to another window OR minimized — and hide
  // it when the app regains focus or the meeting is left. Event-driven (no
  // polling) via the window focus change.
  useEffect(() => {
    if (ended) return;
    const appWindow = getCurrentWindow();
    let disposed = false;
    let unlisten: (() => void) | undefined;
    appWindow
      .onFocusChanged(({ payload: focused }) => {
        if (disposed) return;
        if (focused) {
          void invoke("hide_meeting_pill").catch(() => {});
        } else {
          void invoke("show_meeting_pill").catch(() => {});
        }
      })
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(() => {
        /* window API unavailable */
      });
    return () => {
      disposed = true;
      unlisten?.();
      void invoke("hide_meeting_pill").catch(() => {});
    };
  }, [ended]);

  // Load this meeting's saved notes once.
  useEffect(() => {
    let cancelled = false;
    invoke<string>("meeting_engine_get_notes", { meetingId })
      .then((value) => {
        if (!cancelled) setNotes(value ?? "");
      })
      .catch(() => {
        /* no notes yet */
      });
    return () => {
      cancelled = true;
    };
  }, [meetingId]);

  // Debounced notes autosave (same store the Meetings page + AI chat read from).
  const handleNotesChange = useCallback(
    (value: string) => {
      setNotes(value);
      setNotesStatus("saving");
      if (notesSaveTimer.current) window.clearTimeout(notesSaveTimer.current);
      notesSaveTimer.current = window.setTimeout(() => {
        invoke("meeting_engine_set_notes", { meetingId, notes: value })
          .then(() => setNotesStatus("saved"))
          .catch(() => setNotesStatus("idle"));
      }, 600);
    },
    [meetingId],
  );

  // Auto-scroll to bottom when new chunks arrive
  useEffect(() => {
    transcriptEndRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [chunks]);

  // ── Meeting engine session (start on mount, stop on unmount) ────────────
  useEffect(() => {
    let cancelled = false;
    invoke<MeetingEngineStatus>("meeting_engine_start_session", { meetingId })
      .then((status) => {
        if (!cancelled) {
          applyMeetingStatus(status);
        }
      })
      .catch((e) => {
        console.warn("[meeting_engine] start failed:", e);
      });

    return () => {
      cancelled = true;
      invoke("meeting_engine_stop_session").catch(() => {});
      setEngineRunning(false);
      setCaptureRunning(false);
      setMicTrackActive(false);
      setSystemTrackActive(false);
      setLocalSpeechActive(false);
      setEngineError(null);
      setTranscriptionRunning(false);
    };
  }, [applyMeetingStatus, meetingId]);

  useEffect(() => {
    const unlistenPromise = listen<MeetingEngineStatus>("meeting-engine-state", (event) => {
      applyMeetingStatus(event.payload);
    });

    invoke<MeetingEngineStatus>("meeting_engine_get_status")
      .then(applyMeetingStatus)
      .catch(() => {});

    return () => {
      unlistenPromise.then((fn) => fn());
    };
  }, [applyMeetingStatus]);

  useEffect(() => {
    let cancelled = false;

    const mergeLivePayload = (payload: MeetingLiveTranscriptPayload) => {
      if (cancelled || !payload.chunks?.length) return;
      if (
        payload.session_id
        && engineSessionIdRef.current
        && payload.session_id !== engineSessionIdRef.current
      ) {
        return;
      }
      setChunks((prev) =>
        mergeTranscriptChunks(prev, payload.chunks.map(liveChunkToTranscriptChunk))
      );
    };

    const unlistenPromise = listen<MeetingLiveTranscriptEvent>(
      "meeting-engine-live-transcript",
      (event) => {
        if (cancelled) return;
        if (
          engineSessionIdRef.current
          && event.payload.session_id !== engineSessionIdRef.current
        ) {
          return;
        }
        setChunks((prev) =>
          mergeTranscriptChunks(prev, [liveChunkToTranscriptChunk(event.payload.chunk)])
        );
      },
    );

    invoke<MeetingLiveTranscriptPayload>("meeting_engine_get_live_transcript")
      .then(mergeLivePayload)
      .catch(() => {});

    return () => {
      cancelled = true;
      unlistenPromise.then((fn) => fn());
    };
  }, []);

  useEffect(() => {
    const cleanupRunning = transcriptCleanupStatus === "running";
    const finalDiarizationRunning =
      finalDiarizationStatus === "running" || transcriptionStatus === "final_diarizing";
    if (!ended && !transcriptionRunning && !cleanupRunning && !finalDiarizationRunning) return;
    const terminalTranscription =
      transcriptionStatus === "completed"
      || transcriptionStatus === "failed"
      || transcriptionStatus.startsWith("skipped_");
    if (!cleanupRunning && !finalDiarizationRunning && terminalTranscription) return;

    const id = setInterval(() => {
      invoke<MeetingEngineStatus>("meeting_engine_get_status")
        .then(applyMeetingStatus)
        .catch(() => {});
    }, 1000);

    return () => clearInterval(id);
  }, [
    applyMeetingStatus,
    ended,
    finalDiarizationStatus,
    transcriptionRunning,
    transcriptionStatus,
    transcriptCleanupStatus,
  ]);

  // ── Toggle mute handler ─────────────────────────────────────────────────
  const handleToggleMute = useCallback(async () => {
    setControlBusy(true);
    setControlError(null);
    try {
      const status = await invoke<MeetingEngineStatus>("meeting_engine_toggle_mute");
      applyMeetingStatus(status);
    } catch (e) {
      console.warn("[meeting_engine] toggle mute failed:", e);
      setControlError(e instanceof Error ? e.message : String(e));
    } finally {
      setControlBusy(false);
    }
  }, [applyMeetingStatus]);

  const handleLeave = useCallback(async () => {
    setControlBusy(true);
    setControlError(null);
    try {
      const status = await invoke<MeetingEngineStatus>("meeting_engine_stop_session");
      applyMeetingStatus(status);
      setEnded(true);
    } catch (e) {
      setControlError(e instanceof Error ? e.message : String(e));
    }
    setControlBusy(false);
  }, [applyMeetingStatus]);

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
      const status = await invoke<MeetingEngineStatus>("meeting_engine_stop_session").catch(() => null);
      if (status) applyMeetingStatus(status);
      setEnded(true);
    } catch (e) {
      setControlError(e instanceof Error ? e.message : String(e));
    } finally {
      setEnding(false);
    }
  }, [applyMeetingStatus, ending, meetingId]);

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
          invoke("meeting_engine_stop_session").catch(() => {});
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
                return mergeTranscriptChunks(prev, msg.current_chunks);
              });
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
              setChunks((prev) => mergeTranscriptChunks(prev, [newChunk]));
              break;
            }

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
              invoke("meeting_engine_stop_session").catch(() => {});
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

  // Format timestamp

  // Active participant count
  const activeParticipants = participants.filter((p) => p.status !== "left").length;
  const conn = getConnection();
  const isOwner = !!meeting && !!conn && meeting.created_by === conn.accountId;
  const captureLabel = !engineRunning
    ? transcriptionRunning
      ? "Transcribing"
      : "Stopped"
    : engineError
      ? "Error"
      : muted
        ? "Muted"
        : captureRunning
          ? "Recording"
          : "Ready";
  const engineReady = (engineRunning && !muted && !engineError) || transcriptionRunning;
  const systemUnavailable = systemCaptureStatus === "unavailable" || Boolean(systemCaptureError);
  const engineStatusLabel = engineError
    ? "Mic error"
    : transcriptionRunning
      ? "Transcribing"
    : muted
    ? "Engine muted"
    : captureRunning
      ? micTrackActive && systemTrackActive
        ? "Mic + system"
        : micTrackActive
          ? systemUnavailable
            ? "Mic only"
            : "Mic recording"
          : systemTrackActive
            ? "System audio"
            : localSpeechActive
              ? "Local speech"
              : "Audio capture"
      : engineRunning
        ? systemUnavailable
          ? "System unavailable"
          : "Engine ready"
        : "Engine stopped";
  const rawTranscript = transcriptText?.trim() || "";
  const cleanedTranscript = transcriptCleanedText?.trim() || "";
  const finalTranscript = finalTranscriptText?.trim() || "";
  const liveTranscriptOverride = useMemo(() => {
    if (chunks.length === 0) return "";
    return chunks
      .map((chunk) => `[${formatTimestamp(chunk.timestamp_ms)} ${chunk.speaker_name}] ${chunk.text}`)
      .join("\n");
  }, [chunks]);
  const hasFinalTranscript = finalDiarizationStatus === "completed" && finalTranscript.length > 0;
  const hasCleanedTranscript = transcriptCleanupStatus === "completed" && cleanedTranscript.length > 0;
  const activeTranscriptText = transcriptReviewMode === "final" && hasFinalTranscript
    ? finalTranscript
    : transcriptReviewMode === "cleaned" && hasCleanedTranscript
      ? cleanedTranscript
      : rawTranscript;
  const finalizationActive = finalDiarizationStatus === "running" || transcriptionStatus === "final_diarizing";
  const cleanupActive = transcriptCleanupStatus === "running" || transcriptionStatus === "cleaning";
  const intelligenceSourceKey = hasFinalTranscript
    ? `final:${finalTranscriptJsonPath || finalTranscript.length}`
    : finalizationActive
      ? ""
      : hasCleanedTranscript
        ? `cleaned:${cleanedTranscript.length}`
        : rawTranscript.length > 0
          ? `raw:${rawTranscript.length}`
          : "";
  const meetingAiRunning = meetingAiStatus === "running";
  const liveChatTranscript = liveTranscriptOverride.trim();
  const chatCanSend = ended
    ? Boolean(activeTranscriptText || rawTranscript)
    : Boolean(liveChatTranscript);
  const liveTranscriptModelLabel = liveTranscriptModel
    ? liveTranscriptModel.split(/[\\/]/).pop() || liveTranscriptModel
    : liveTranscriptStatus.replace(/_/g, " ");
  const liveChatUnavailableLabel = ended
    ? "Transcript is not ready yet."
    : liveTranscriptError
      ? `Live transcript unavailable: ${liveTranscriptError}`
      : liveTranscriptStatus === "disabled"
        ? "Live transcript is disabled."
        : liveTranscriptStatus.startsWith("skipped")
          ? "Live transcript could not start. Check whisper.cpp settings."
            : liveTranscriptStatus === "running" || liveTranscriptStatus === "running_with_errors"
              ? `Listening with ${liveTranscriptModelLabel}; waiting for the first transcript chunk.`
              : "Live chat starts after transcript chunks arrive.";

  const applyMeetingIntelligenceResult = useCallback((result: MeetingIntelligenceResult) => {
    setMeetingAiStatus(result.status || "completed");
    setMeetingAiSummary(result.summary || "");
    setMeetingAiActionItems(result.action_items || []);
    setMeetingAiDecisions(result.decisions || []);
    setMeetingAiSource(result.transcript_source || null);
    setMeetingAiModel(result.model || null);
    setMeetingAiLatencyMs(result.latency_ms ?? null);
    setMeetingAiError(null);
  }, []);

  const loadCachedMeetingIntelligence = useCallback(async () => {
    try {
      const result = await invoke<MeetingIntelligenceResult | null>(
        "meeting_engine_get_cached_intelligence",
        { meetingId },
      );
      if (!result?.summary && !result?.action_items?.length && !result?.decisions?.length) {
        return false;
      }
      applyMeetingIntelligenceResult(result);
      lastIntelligenceKeyRef.current = `cached:${meetingId}:${result.model}:${result.latency_ms}`;
      return true;
    } catch (e) {
      setMeetingAiError(e instanceof Error ? e.message : String(e));
      return false;
    }
  }, [applyMeetingIntelligenceResult, meetingId]);

  const generateMeetingIntelligence = useCallback(async (force = false) => {
    if (!intelligenceSourceKey) return;
    if (!force && lastIntelligenceKeyRef.current === intelligenceSourceKey) return;
    lastIntelligenceKeyRef.current = intelligenceSourceKey;
    setMeetingAiStatus("running");
    setMeetingAiError(null);
    try {
      const result = await invoke<MeetingIntelligenceResult>("meeting_engine_generate_intelligence", {
        meetingId,
      });
      applyMeetingIntelligenceResult(result);
    } catch (e) {
      lastIntelligenceKeyRef.current = null;
      setMeetingAiStatus("failed");
      setMeetingAiError(e instanceof Error ? e.message : String(e));
    }
  }, [applyMeetingIntelligenceResult, intelligenceSourceKey]);

  useEffect(() => {
    if (!ended || meetingAiRunning || meetingAiSummary.trim()) {
      return;
    }
    void loadCachedMeetingIntelligence();
  }, [ended, loadCachedMeetingIntelligence, meetingAiRunning, meetingAiSummary]);

  useEffect(() => {
    if (!ended || !intelligenceSourceKey || transcriptionRunning || finalizationActive || cleanupActive) {
      return;
    }
    void generateMeetingIntelligence(false);
  }, [
    cleanupActive,
    ended,
    finalizationActive,
    generateMeetingIntelligence,
    intelligenceSourceKey,
    transcriptionRunning,
  ]);

  useEffect(() => {
    if (hasFinalTranscript) {
      setTranscriptReviewMode("final");
      return;
    }
    if (!hasCleanedTranscript && transcriptReviewMode !== "raw") {
      setTranscriptReviewMode("raw");
    } else if (transcriptReviewMode === "final") {
      setTranscriptReviewMode(hasCleanedTranscript ? "cleaned" : "raw");
    }
  }, [hasCleanedTranscript, hasFinalTranscript, transcriptReviewMode]);

  const aiChatPanel = (
    <div
      className="absolute right-4 top-4 bottom-4 z-40 w-[380px] max-w-[calc(100%-2rem)] rounded-xl flex flex-col overflow-hidden"
      style={{
        background: "hsl(var(--surface-2) / 0.98)",
        border: "1px solid hsl(var(--surface-4))",
        boxShadow: "0 24px 70px hsl(0 0% 0% / 0.48)",
        backdropFilter: "blur(28px) saturate(170%)",
        WebkitBackdropFilter: "blur(28px) saturate(170%)",
      }}
    >
      <div
        className="flex items-center justify-between px-4 py-3"
        style={{ borderBottom: "1px solid hsl(var(--surface-4))" }}
      >
        <div className="flex items-center gap-2 min-w-0">
          <Sparkles size={15} style={{ color: "hsl(var(--primary))" }} />
          <div className="min-w-0">
            <p className="text-[12px] font-bold text-foreground">Meeting AI Chat</p>
            <p className="text-[10px] text-muted-foreground truncate">
              {ended ? "Final meeting context" : "Live context so far"}
            </p>
          </div>
        </div>
        <button
          type="button"
          onClick={() => setAiChatOpen(false)}
          className="flex items-center justify-center w-8 h-8 rounded-lg transition-colors"
          style={{ background: "hsl(var(--surface-4))", color: "hsl(var(--foreground))" }}
          title="Close AI chat"
        >
          <X size={14} />
        </button>
      </div>

      <div className="min-h-0 flex-1">
        <MeetingAiChat
          resetKey={meetingId}
          summary={meetingAiSummary || null}
          transcriptOverride={ended ? null : liveChatTranscript}
          notes={notes || null}
          canSend={chatCanSend}
          unavailableLabel={liveChatUnavailableLabel}
          emptyHint={'Ask anything from this meeting — "What are the decisions?", "What did I say?", "What should I do next?"'}
          placeholder={ended ? "Ask about this meeting" : "Ask about the live meeting"}
        />
      </div>
    </div>
  );

  if (ended) {
    // Meeting ended — the parent navigates to the Meetings page and focuses
    // this meeting (see onEnded). Render a brief hand-off state for the single
    // frame before this view unmounts. The entire post-meeting experience
    // (summary, transcript, actions, AI chat, live processing stages) lives in
    // MeetingsView — the single source of truth. No duplicate notes page here.
    return (
      <div className="h-full flex items-center justify-center">
        <div className="flex items-center gap-2 text-[13px] text-muted-foreground">
          <Loader2 size={16} className="animate-spin" />
          <span>Wrapping up your meeting…</span>
        </div>
      </div>
    );
  }

  // ── Main layout ────────────────────────────────────────────────────────────

  return (
    <div className="h-full flex flex-col overflow-hidden relative">
      <button
        type="button"
        onClick={() => setAiChatOpen(true)}
        className="absolute top-3 left-1/2 -translate-x-1/2 z-30 h-11 px-5 rounded-full flex items-center gap-2 text-[12px] font-bold transition-transform hover:scale-[1.02]"
        style={{
          background: "hsl(var(--surface-2) / 0.96)",
          border: "1px solid hsl(var(--surface-4))",
          color: "hsl(var(--foreground))",
          boxShadow: "0 14px 38px hsl(0 0% 0% / 0.35), 0 0 0 4px hsl(var(--primary) / 0.08)",
          backdropFilter: "blur(24px) saturate(170%)",
          WebkitBackdropFilter: "blur(24px) saturate(170%)",
        }}
        title="Open live meeting AI chat"
      >
        <Sparkles size={16} style={{ color: "hsl(var(--primary))" }} />
        <span>{chunks.length > 0 ? formatTimestamp(chunks[chunks.length - 1].timestamp_ms) : "00:00"}</span>
      </button>
      {aiChatOpen && aiChatPanel}

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
                  {liveChatUnavailableLabel}
                </p>
              </div>
            ) : (
              <div className="space-y-3">
                {chunks.map((chunk, i) => {
                  const color = speakerColor(chunk.speaker_id);
                  // Group consecutive chunks from same speaker
                  const prevChunk = i > 0 ? chunks[i - 1] : null;
                  const sameSpeaker = prevChunk?.speaker_id === chunk.speaker_id;

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
                            {formatTimestamp(chunk.timestamp_ms)}
                          </span>
                        </div>
                      )}
                      <p className="text-[12px] text-foreground leading-relaxed pl-0">
                        {chunk.text}
                      </p>
                    </div>
                  );
                })}
                <div ref={transcriptEndRef} />
              </div>
            )}
          </div>
        </div>

        {/* Right panel — Notes */}
        <div className="flex-[2] flex flex-col overflow-hidden min-w-0">
          <div className="flex flex-shrink-0 items-center justify-between px-4 py-3">
            <h2 className="text-[12px] font-semibold uppercase tracking-wide text-muted-foreground">
              Notes
            </h2>
            <span className="text-[10px] text-muted-foreground">
              {notesStatus === "saving" ? "Saving…" : notesStatus === "saved" ? "Saved" : ""}
            </span>
          </div>
          <div className="min-h-0 flex-1 px-4 pb-24">
            <textarea
              value={notes}
              onChange={(e) => handleNotesChange(e.target.value)}
              placeholder="Jot notes during the meeting…  Saved to this meeting and used as context in AI chat."
              spellCheck={false}
              className="h-full w-full resize-none rounded-xl px-4 py-3 text-[13px] leading-relaxed text-foreground placeholder:text-muted-foreground/50 focus:outline-none"
              style={{
                background: "hsl(var(--surface-2))",
                border: "1px solid hsl(var(--surface-4))",
              }}
            />
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
            disabled={!engineRunning || controlBusy}
            className="flex items-center gap-2 h-10 px-4 rounded-full text-[12px] font-semibold transition-all disabled:opacity-40"
            style={{
              background: captureRunning
                ? "hsl(142 70% 45% / 0.18)"
                : muted
                  ? "hsl(354 80% 55% / 0.16)"
                  : "hsl(226 80% 60% / 0.16)",
              color: captureRunning
                ? "hsl(142 70% 65%)"
                : muted
                  ? "hsl(354 85% 75%)"
                  : "hsl(226 80% 78%)",
              boxShadow: captureRunning
                ? "0 0 18px hsl(142 70% 55% / 0.24)"
                : "none",
            }}
            title={muted ? "Resume meeting session" : "Mute meeting session"}
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
            className="flex items-center gap-1.5 h-8 px-3 rounded-full text-[11px] font-medium"
            title={
              engineError || transcriptionError || systemCaptureError || transcriptionStatus || lastGateReason
            }
            style={{
              background: engineReady
                ? "hsl(142 70% 45% / 0.12)"
                : "hsl(354 80% 55% / 0.16)",
              color: engineReady
                ? "hsl(142 70% 65%)"
                : "hsl(354 85% 75%)",
            }}
          >
            <span
              className={captureRunning ? "w-1.5 h-1.5 rounded-full animate-pulse" : "w-1.5 h-1.5 rounded-full"}
              style={{
                background: engineReady
                  ? "hsl(142 70% 55%)"
                  : "hsl(354 80% 65%)",
              }}
            />
            <span>{engineStatusLabel}</span>
          </div>

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
