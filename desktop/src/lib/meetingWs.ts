/**
 * WebSocket client for Said Enterprise meeting transcript streaming.
 *
 * Usage:
 *   const ws = new MeetingWebSocket(serverUrl, meetingId, jwt);
 *   ws.onMessage = (msg) => { ... };
 *   ws.onStatusChange = (status) => { ... };
 *   ws.connect();
 *   ws.sendChunk("Hello world", Date.now());
 *   ws.disconnect();
 */

export type ConnectionStatus = "disconnected" | "connecting" | "connected" | "reconnecting";

export interface TranscriptChunkMessage {
  type: "transcript_chunk";
  speaker_id: string;
  speaker_name: string;
  text: string;
  timestamp_ms: number;
  chunk_index: number;
}

export interface ParticipantJoinedMessage {
  type: "participant_joined";
  account_id: string;
  email: string;
  display_name?: string;
}

export interface ParticipantLeftMessage {
  type: "participant_left";
  account_id: string;
}

export interface TaskDetectedMessage {
  type: "task_detected";
  task_id: string;
  title: string;
  assignee_id?: string;
  assignee_name?: string;
}

export interface SummaryUpdatedMessage {
  type: "summary_updated";
  summary: string;
}

export interface ConnectedMessage {
  type: "connected";
  meeting_id: string;
  account_id: string;
}

export interface PongMessage {
  type: "pong";
}

export interface ResumeMessage {
  type: "resume";
  last_chunk_index: number;
}

export interface CatchupMessage {
  type: "catchup";
  summary: string | null;
  current_chunks: TranscriptChunkMessage[];
  tasks: Array<{ task_id: string; title: string; assignee_name?: string }>;
  decisions: Array<{ text: string }>;
  participants: Array<{ account_id: string; email: string; display_name?: string; status: string }>;
}

export type ServerMessage =
  | TranscriptChunkMessage
  | ParticipantJoinedMessage
  | ParticipantLeftMessage
  | TaskDetectedMessage
  | SummaryUpdatedMessage
  | ConnectedMessage
  | PongMessage
  | ResumeMessage
  | CatchupMessage;

export class MeetingWebSocket {
  private ws: WebSocket | null = null;
  private serverUrl: string;
  private meetingId: string;
  private jwt: string;
  private status: ConnectionStatus = "disconnected";
  private reconnectAttempts = 0;
  private maxReconnectAttempts = 10;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private pingTimer: ReturnType<typeof setInterval> | null = null;
  private intentionalClose = false;
  private pendingChunks: Array<{ text: string; timestamp_ms: number }> = [];
  private lastChunkIndex: number = -1;

  onMessage: ((msg: ServerMessage) => void) | null = null;
  onStatusChange: ((status: ConnectionStatus) => void) | null = null;

  constructor(serverUrl: string, meetingId: string, jwt: string) {
    this.serverUrl = serverUrl.replace(/\/+$/, "").replace(/^http/, "ws");
    this.meetingId = meetingId;
    this.jwt = jwt;
  }

  connect(): void {
    if (this.ws) return;
    this.intentionalClose = false;
    this.setStatus("connecting");

    const url = `${this.serverUrl}/v1/meetings/${this.meetingId}/ws?token=${encodeURIComponent(this.jwt)}`;
    this.ws = new WebSocket(url);

    this.ws.onopen = () => {
      this.reconnectAttempts = 0;
      this.setStatus("connected");
      this.startPing();

      // On reconnect, send resume message so the server can fill gaps
      if (this.lastChunkIndex >= 0 && this.ws?.readyState === WebSocket.OPEN) {
        this.ws.send(JSON.stringify({
          type: "resume",
          last_chunk_index: this.lastChunkIndex,
        }));
      }

      // Flush any chunks buffered while disconnected
      if (this.pendingChunks.length > 0 && this.ws?.readyState === WebSocket.OPEN) {
        for (const chunk of this.pendingChunks) {
          this.ws.send(JSON.stringify({
            type: "transcript_chunk",
            text: chunk.text,
            timestamp_ms: chunk.timestamp_ms,
          }));
        }
        this.pendingChunks = [];
      }
    };

    this.ws.onmessage = (event) => {
      try {
        const msg = JSON.parse(event.data) as ServerMessage;
        // Track the latest chunk_index for resume protocol
        if (msg.type === "transcript_chunk") {
          this.lastChunkIndex = msg.chunk_index;
        }
        this.onMessage?.(msg);
      } catch {
        // ignore unparseable messages
      }
    };

    this.ws.onclose = () => {
      this.cleanup();
      if (!this.intentionalClose) {
        this.attemptReconnect();
      } else {
        this.setStatus("disconnected");
      }
    };

    this.ws.onerror = () => {
      // onclose will fire after onerror
    };
  }

  disconnect(): void {
    this.intentionalClose = true;
    this.cleanup();
    this.setStatus("disconnected");
  }

  sendChunk(text: string, timestampMs: number): void {
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) {
      // Buffer chunks while disconnected so they can be flushed on reconnect
      this.pendingChunks.push({ text, timestamp_ms: timestampMs });
      return;
    }
    this.ws.send(JSON.stringify({
      type: "transcript_chunk",
      text,
      timestamp_ms: timestampMs,
    }));
  }

  getStatus(): ConnectionStatus {
    return this.status;
  }

  private setStatus(status: ConnectionStatus): void {
    this.status = status;
    this.onStatusChange?.(status);
  }

  private startPing(): void {
    this.stopPing();
    this.pingTimer = setInterval(() => {
      if (this.ws?.readyState === WebSocket.OPEN) {
        this.ws.send(JSON.stringify({ type: "ping" }));
      }
    }, 30_000);
  }

  private stopPing(): void {
    if (this.pingTimer) {
      clearInterval(this.pingTimer);
      this.pingTimer = null;
    }
  }

  private cleanup(): void {
    this.stopPing();
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    if (this.ws) {
      this.ws.onopen = null;
      this.ws.onmessage = null;
      this.ws.onclose = null;
      this.ws.onerror = null;
      if (this.ws.readyState === WebSocket.OPEN || this.ws.readyState === WebSocket.CONNECTING) {
        this.ws.close();
      }
      this.ws = null;
    }
  }

  private attemptReconnect(): void {
    if (this.reconnectAttempts >= this.maxReconnectAttempts) {
      this.setStatus("disconnected");
      return;
    }
    this.setStatus("reconnecting");
    this.reconnectAttempts++;
    // Exponential backoff: 1s, 2s, 4s, 8s... capped at 30s
    const delay = Math.min(1000 * Math.pow(2, this.reconnectAttempts - 1), 30_000);
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      this.connect();
    }, delay);
  }
}
