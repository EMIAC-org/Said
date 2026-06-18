import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Loader2 } from "lucide-react";
import { ChatThinkingDots, MeetingRichText } from "@/lib/meetingMarkdown";

export interface MeetingChatResult {
  status: string;
  provider: string;
  model: string;
  latency_ms: number;
  transcript_source: string;
  answer: string;
}

type ChatMessage = { role: "user" | "assistant"; text: string; meta?: string; streaming?: boolean };

interface MeetingAiChatProps {
  /** Generated summary to ground answers (null if none yet). */
  summary: string | null;
  /** Live/raw transcript override; null falls back to the engine's cached final transcript. */
  transcriptOverride: string | null;
  /** The user's personal notes for this meeting, passed as extra chat context. */
  notes?: string | null;
  /** Whether sending is allowed right now (e.g. transcript ready). */
  canSend: boolean;
  /** Message shown when a send is attempted while !canSend. */
  unavailableLabel?: string;
  /** Empty-state hint. */
  emptyHint?: string;
  /** Input placeholder. */
  placeholder?: string;
  /** Clicking a [mm:ss] timestamp in an answer seeks the recording. */
  onSeek?: (seconds: number) => void;
  /** Reset the conversation when this key changes (e.g. selected meeting id). */
  resetKey?: string;
  /** Tauri command to invoke (default "meeting_engine_chat"). Lets the digest
   *  view reuse this component with "meeting_engine_digest_chat". */
  chatCommand?: string;
  /** Extra args merged into the invoke payload (e.g. digest `refs`). */
  chatArgs?: Record<string, unknown>;
}

/**
 * Single, shared Meeting AI chat used by both the live-meeting view and the
 * post-meeting detail view. Streams the answer token-by-token over the
 * `meeting-chat-delta` event and renders it as formatted markdown with clickable
 * timestamps. Always sends a `request_id` (the streaming command requires it).
 */
export function MeetingAiChat({
  summary,
  transcriptOverride,
  notes,
  canSend,
  unavailableLabel,
  emptyHint,
  placeholder,
  onSeek,
  resetKey,
  chatCommand,
  chatArgs,
}: MeetingAiChatProps) {
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const scrollRef = useRef<HTMLDivElement | null>(null);

  // Guards so an in-flight chat can't write into the wrong (or unmounted)
  // conversation after a meeting switch, and its event listener can't leak.
  const mountedRef = useRef(true);
  const resetKeyRef = useRef(resetKey);
  resetKeyRef.current = resetKey;
  const activeUnlistenRef = useRef<null | (() => void)>(null);
  useEffect(() => {
    // Re-arm on (re)mount. Without this, React StrictMode's dev mount→unmount→
    // mount cycle leaves mountedRef.current=false (the first unmount set it
    // false and the second mount never reset it), so isCurrent() is permanently
    // false and EVERY chat answer is dropped — the UI stays stuck on "thinking".
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      activeUnlistenRef.current?.();
      activeUnlistenRef.current = null;
    };
  }, []);

  // Reset the conversation when the underlying meeting changes — and cancel any
  // in-flight send's UI state + listener so a late answer can't land here.
  useEffect(() => {
    setMessages([]);
    setDraft("");
    setError(null);
    setBusy(false);
    activeUnlistenRef.current?.();
    activeUnlistenRef.current = null;
  }, [resetKey]);

  // Keep the latest grounding values for the async send without re-creating it.
  const groundingRef = useRef({ summary, transcriptOverride, notes, chatCommand, chatArgs });
  groundingRef.current = { summary, transcriptOverride, notes, chatCommand, chatArgs };

  useEffect(() => {
    scrollRef.current?.scrollTo({ top: scrollRef.current.scrollHeight });
  }, [messages]);

  const send = useCallback(async () => {
    const question = draft.trim();
    if (!question || busy) return;
    if (!canSend) {
      setError(unavailableLabel ?? "Not ready yet.");
      return;
    }
    const requestId = `${Date.now()}-${Math.random().toString(36).slice(2)}`;
    // The meeting this send belongs to. If it changes (or we unmount) before the
    // answer arrives, every handler below bails so the answer can't bleed into a
    // different conversation.
    const sendKey = resetKeyRef.current;
    const isCurrent = () => mountedRef.current && resetKeyRef.current === sendKey;
    setDraft("");
    setError(null);
    setBusy(true);
    setMessages((prev) => [
      ...prev,
      { role: "user", text: question },
      { role: "assistant", text: "", streaming: true },
    ]);

    const unlisten = await listen<{ request_id: string; delta: string }>(
      "meeting-chat-delta",
      (event) => {
        if (event.payload.request_id !== requestId || !isCurrent()) return;
        setMessages((prev) => {
          const next = prev.slice();
          const last = next[next.length - 1];
          if (last && last.role === "assistant" && last.streaming) {
            next[next.length - 1] = { ...last, text: last.text + event.payload.delta };
          }
          return next;
        });
      },
    );
    activeUnlistenRef.current = unlisten;

    try {
      const grounding = groundingRef.current;
      const result = await invoke<MeetingChatResult>(grounding.chatCommand ?? "meeting_engine_chat", {
        requestId,
        question,
        summary: grounding.summary ?? null,
        transcriptOverride: grounding.transcriptOverride ?? null,
        notes: grounding.notes ?? null,
        ...(grounding.chatArgs ?? {}),
      });
      if (!isCurrent()) return;
      setMessages((prev) => {
        const next = prev.slice();
        const last = next[next.length - 1];
        if (!last || last.role !== "assistant" || !last.streaming) return prev;
        next[next.length - 1] = {
          role: "assistant",
          text: result.answer,
          meta: `${result.transcript_source} · ${result.model} · ${(result.latency_ms / 1000).toFixed(1)}s`,
        };
        return next;
      });
    } catch (e) {
      if (!isCurrent()) return;
      setError(e instanceof Error ? e.message : String(e));
      setMessages((prev) => {
        const next = prev.slice();
        const last = next[next.length - 1];
        if (last && last.role === "assistant" && last.streaming) next.pop();
        return next;
      });
    } finally {
      unlisten();
      if (activeUnlistenRef.current === unlisten) activeUnlistenRef.current = null;
      if (isCurrent()) setBusy(false);
    }
  }, [draft, busy, canSend, unavailableLabel]);

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div ref={scrollRef} className="min-h-0 flex-1 space-y-4 overflow-y-auto p-5">
        {messages.length === 0 ? (
          <p className="text-[14px] text-muted-foreground">
            {emptyHint ?? "Ask about this recording. Answers use the meeting transcript and generated summary."}
          </p>
        ) : (
          messages.map((message, index) => (
            <div
              key={`${message.role}-${index}`}
              className="max-w-[88%] rounded-xl px-4 py-3"
              style={{
                marginLeft: message.role === "user" ? "auto" : 0,
                background: message.role === "user" ? "hsl(var(--primary) / 0.16)" : "hsl(0 0% 0% / 0.22)",
                border: "1px solid hsl(var(--surface-4))",
              }}
            >
              <p className="text-[11px] font-bold uppercase tracking-[0.14em] text-muted-foreground">
                {message.role === "user" ? "You" : "AirNote"}
              </p>
              {message.role === "assistant" ? (
                message.text ? (
                  <MeetingRichText text={message.text} onSeek={onSeek} />
                ) : message.streaming ? (
                  <ChatThinkingDots />
                ) : null
              ) : (
                <p className="mt-2 whitespace-pre-wrap text-[14px] leading-7 text-foreground">{message.text}</p>
              )}
              {message.meta ? <p className="mt-2 text-[11px] text-muted-foreground">{message.meta}</p> : null}
            </div>
          ))
        )}
        {error ? <p className="text-[12px]" style={{ color: "hsl(354 85% 75%)" }}>{error}</p> : null}
      </div>
      <div className="flex gap-3 border-t p-4" style={{ borderColor: "hsl(var(--surface-4))" }}>
        <input
          value={draft}
          onChange={(event) => setDraft(event.currentTarget.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter" && !event.shiftKey) {
              event.preventDefault();
              void send();
            }
          }}
          placeholder={placeholder ?? "Ask about this meeting…"}
          className="h-11 min-w-0 flex-1 rounded-lg bg-transparent px-4 text-[14px] outline-none"
          style={{ border: "1px solid hsl(var(--surface-4))", color: "hsl(var(--foreground))" }}
        />
        <button
          type="button"
          onClick={() => void send()}
          disabled={!draft.trim() || busy || !canSend}
          className="h-11 rounded-lg px-4 text-[12px] font-bold disabled:opacity-45"
          style={{ background: "hsl(var(--primary))", color: "hsl(var(--primary-foreground))" }}
        >
          {busy ? <Loader2 size={15} className="animate-spin" /> : "Send"}
        </button>
      </div>
    </div>
  );
}
