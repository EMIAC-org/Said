import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { formatTimestamp, speakerColor } from "@/lib/meetingFormat";
import type { MutableRefObject, ReactNode } from "react";
import {
  AlertTriangle,
  Ban,
  Check,
  ChevronDown,
  Copy,
  Download,
  Eraser,
  ExternalLink,
  FileText,
  Layers,
  ListChecks,
  Loader2,
  MessageSquare,
  Pause,
  Pencil,
  Play,
  Plus,
  RefreshCw,
  RotateCcw,
  RotateCw,
  ScrollText,
  Search,
  Sparkles,
  Star,
  Trash2,
  Video,
  X,
} from "lucide-react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { exportMeetingToLark } from "@/lib/enterprise";
import { openExternal } from "@/lib/invoke";
import { NEW_MODEL_FILE } from "@/lib/onDeviceModel";
import { MeetingAiChat } from "@/components/MeetingAiChat";
import { Skeleton } from "@/components/ui/skeleton";
import { DigestView } from "@/components/views/DigestView";
import {
  MeetingRichText,
  MeetingSummaryContent,
  stripInlineMarkdown,
  summaryLead,
} from "@/lib/meetingMarkdown";

interface Meeting {
  id: string;
  title: string;
  status: "scheduled" | "live" | "ended";
  scheduled_at?: string | null;
  agenda?: string | null;
  participants_count?: number;
  created_at: string;
}

interface MeetingOverview {
  title?: string | null;
  tags: string[];
  action_count: number;
  decision_count: number;
  word_count: number;
  has_intelligence: boolean;
  favorite: boolean;
  hidden: boolean;
  has_local_files: boolean;
  lark_doc_url?: string | null;
}

// Reject if `promise` doesn't settle within `ms`. Used to bound Tauri invokes
// that can be orphaned by a webview reload and otherwise never resolve. The
// underlying promise is left to dangle (harmless) — we just stop awaiting it.
function withTimeout<T>(promise: Promise<T>, ms: number, message: string): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(message)), ms);
    promise.then(
      (value) => { clearTimeout(timer); resolve(value); },
      (err) => { clearTimeout(timer); reject(err); },
    );
  });
}

// Local meeting list entry from `meeting_engine_list_meetings` — meetings are a
// single-device, on-device feature now (no control-plane list).
interface LocalMeetingSummary {
  id: string;
  title: string;
  status: "live" | "ended";
  created_at_ms: number;
  tags: string[];
  action_count: number;
  decision_count: number;
  word_count: number;
  has_intelligence: boolean;
  favorite: boolean;
  hidden: boolean;
  has_local_files: boolean;
  lark_doc_url?: string | null;
}

function localToMeeting(m: LocalMeetingSummary): Meeting {
  return {
    id: m.id,
    title: m.title,
    status: m.status,
    created_at: new Date(m.created_at_ms).toISOString(),
    scheduled_at: null,
    agenda: null,
    participants_count: 1,
  };
}

function localToOverview(m: LocalMeetingSummary): MeetingOverview {
  return {
    title: m.title,
    tags: m.tags,
    action_count: m.action_count,
    decision_count: m.decision_count,
    word_count: m.word_count,
    has_intelligence: m.has_intelligence,
    favorite: m.favorite,
    hidden: m.hidden,
    has_local_files: m.has_local_files,
    lark_doc_url: m.lark_doc_url ?? null,
  };
}

interface MeetingSearchHit {
  id: string;
  score: number;
  matched_in: string[];
  snippet?: string | null;
}

interface ManualAction {
  title: string;
  done: boolean;
}

type DetailTab = "summary" | "notes" | "transcript" | "actions" | "chat";
type MeetingListFilter = "all" | "favorites" | "today" | "week" | "archived";

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
  title?: string;
  tags?: string[];
  summary: string;
  action_items: MeetingAiActionItem[];
  decisions: MeetingAiDecision[];
}

interface MeetingCachedTranscriptSegment {
  source: string;
  speaker_id: string;
  speaker_name: string;
  start_ms: number;
  end_ms: number;
  text: string;
}

interface MeetingCachedArtifacts {
  meeting_id?: string | null;
  artifact_dir: string;
  audio_path?: string | null;
  audio_duration_ms?: number | null;
  transcript_path?: string | null;
  transcript_source: string;
  transcript: string;
  segments: MeetingCachedTranscriptSegment[];
}



function formatMeetingDate(meeting: Meeting): string {
  const raw = meeting.scheduled_at ?? meeting.created_at;
  if (!raw) return "No date";
  try {
    return new Date(raw).toLocaleDateString(undefined, {
      day: "numeric",
      month: "short",
      year: "numeric",
    });
  } catch {
    return "No date";
  }
}

function meetingTime(meeting: Meeting): Date {
  return new Date(meeting.scheduled_at ?? meeting.created_at);
}

function isSameLocalDay(a: Date, b: Date): boolean {
  return a.getFullYear() === b.getFullYear()
    && a.getMonth() === b.getMonth()
    && a.getDate() === b.getDate();
}

function isInCurrentWeek(date: Date): boolean {
  const now = new Date();
  const day = now.getDay();
  const start = new Date(now);
  start.setHours(0, 0, 0, 0);
  start.setDate(now.getDate() - day);
  const end = new Date(start);
  end.setDate(start.getDate() + 7);
  return date >= start && date < end;
}

function wordCount(text: string | null | undefined): number {
  return text?.trim().split(/\s+/).filter(Boolean).length ?? 0;
}

const TAG_COLORS = [
  "hsl(320 76% 70%)",
  "hsl(154 68% 62%)",
  "hsl(30 86% 62%)",
  "hsl(248 84% 72%)",
  "hsl(205 86% 68%)",
  "hsl(45 92% 64%)",
  "hsl(3 72% 64%)",
  "hsl(180 60% 60%)",
];

// Stable per-label color so a tag keeps the same hue across renders.
function tagColor(label: string): string {
  let hash = 0;
  for (let i = 0; i < label.length; i += 1) {
    hash = (hash * 31 + label.charCodeAt(i)) | 0;
  }
  return TAG_COLORS[Math.abs(hash) % TAG_COLORS.length];
}

function IconButton({
  children,
  label,
  disabled,
  onClick,
}: {
  children: ReactNode;
  label: string;
  disabled?: boolean;
  onClick?: () => void;
}) {
  return (
    <button
      type="button"
      title={label}
      disabled={disabled}
      onClick={onClick}
      className="flex h-8 w-8 items-center justify-center rounded-lg transition-colors disabled:opacity-40"
      style={{
        background: "hsl(var(--surface-3))",
        border: "1px solid hsl(var(--border))",
        color: "hsl(var(--muted-foreground))",
      }}
    >
      {children}
    </button>
  );
}

function ToolbarButton({
  icon,
  label,
  active,
  disabled,
  onClick,
}: {
  icon: ReactNode;
  label: string;
  active?: boolean;
  disabled?: boolean;
  onClick?: () => void;
}) {
  return (
    <button
      type="button"
      disabled={disabled}
      onClick={onClick}
      className="h-9 rounded-lg px-3 text-[12px] font-semibold transition-colors hover:text-foreground disabled:opacity-45"
      style={{
        background: active ? "hsl(var(--chip-lime-bg))" : "hsl(var(--surface-3))",
        border: active ? "1px solid hsl(var(--chip-lime-fg) / 0.26)" : "1px solid hsl(var(--border))",
        color: active ? "hsl(var(--chip-lime-fg))" : "hsl(var(--muted-foreground))",
      }}
    >
      <span className="flex items-center gap-2">
        {icon}
        {label}
      </span>
    </button>
  );
}

/** Copy button with reactive "Copied" feedback. `onCopy` performs the copy; the
 *  button flips to a green ✓ Copied for ~1.5s on success. */
function CopyButton({
  label,
  onCopy,
  disabled,
}: {
  label: string;
  onCopy: () => Promise<void> | void;
  disabled?: boolean;
}) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<number | null>(null);
  useEffect(
    () => () => {
      if (timer.current) window.clearTimeout(timer.current);
    },
    [],
  );
  const handle = async () => {
    try {
      await onCopy();
      setCopied(true);
      if (timer.current) window.clearTimeout(timer.current);
      timer.current = window.setTimeout(() => setCopied(false), 1500);
    } catch {
      /* clipboard denied / nothing to copy */
    }
  };
  return (
    <button
      type="button"
      disabled={disabled}
      onClick={() => void handle()}
      className="h-9 rounded-lg px-3 text-[12px] font-semibold transition-colors hover:text-foreground disabled:opacity-45"
      style={{
        background: copied ? "hsl(var(--chip-lime-bg))" : "hsl(var(--surface-3))",
        border: copied ? "1px solid hsl(var(--chip-lime-fg) / 0.26)" : "1px solid hsl(var(--border))",
        color: copied ? "hsl(var(--chip-lime-fg))" : "hsl(var(--muted-foreground))",
      }}
    >
      <span className="flex items-center gap-2">
        {copied ? <Check size={14} /> : <Copy size={14} />}
        {copied ? "Copied" : label}
      </span>
    </button>
  );
}

function MeetingAudioBar({
  audioSrc,
  audioRef,
  currentTime,
  duration,
  playing,
  speed,
  fallbackDurationMs,
  onDuration,
  onTime,
  onToggle,
  onSeek,
  onSpeed,
  onDownload,
  downloading,
}: {
  audioSrc: string | null;
  audioRef: MutableRefObject<HTMLAudioElement | null>;
  currentTime: number;
  duration: number;
  playing: boolean;
  speed: number;
  fallbackDurationMs?: number | null;
  onDuration: (seconds: number) => void;
  onTime: (seconds: number) => void;
  onToggle: () => void;
  onSeek: (seconds: number) => void;
  onSpeed: () => void;
  onDownload: () => void;
  downloading: boolean;
}) {
  const effectiveDuration = duration || ((fallbackDurationMs ?? 0) / 1000);
  const clampedTime = Math.min(currentTime, Math.max(1, effectiveDuration));
  const pct = effectiveDuration > 0 ? Math.min(100, (clampedTime / effectiveDuration) * 100) : 0;
  return (
    <div
      className="mt-6 flex items-center gap-3 rounded-xl px-3 py-2.5"
      style={{ background: "hsl(var(--surface-3))", border: "1px solid hsl(var(--border))" }}
    >
      {audioSrc ? (
        <audio
          ref={audioRef}
          src={audioSrc}
          preload="metadata"
          onLoadedMetadata={(event) => {
            onDuration(event.currentTarget.duration || 0);
            // Re-apply the chosen rate — a fresh src resets playbackRate to 1.
            event.currentTarget.playbackRate = speed;
          }}
          onTimeUpdate={(event) => onTime(event.currentTarget.currentTime || 0)}
          onEnded={() => onTime(effectiveDuration)}
        />
      ) : null}
      <button
        type="button"
        disabled={!audioSrc}
        onClick={onToggle}
        className="flex h-9 w-9 flex-shrink-0 items-center justify-center rounded-full transition-transform hover:scale-105 disabled:opacity-45 disabled:hover:scale-100"
        style={{ background: "hsl(var(--accent-violet))", color: "hsl(var(--primary-foreground))" }}
        title={playing ? "Pause audio" : "Play audio"}
      >
        {playing ? <Pause size={15} fill="currentColor" /> : <Play size={15} fill="currentColor" className="ml-0.5" />}
      </button>
      <span className="flex-shrink-0 text-[11.5px] font-semibold tabular-nums text-muted-foreground">
        {formatTimestamp(clampedTime * 1000)}
      </span>
      <input
        type="range"
        min={0}
        max={Math.max(1, effectiveDuration)}
        step={0.1}
        value={clampedTime}
        disabled={!audioSrc}
        onChange={(event) => onSeek(Number(event.currentTarget.value))}
        className="audio-range min-w-0 flex-1"
        style={{ background: `linear-gradient(to right, hsl(var(--accent-violet)) ${pct}%, hsl(var(--surface-4)) ${pct}%)` }}
        aria-label="Audio timeline"
      />
      <span className="flex-shrink-0 text-[11.5px] font-semibold tabular-nums text-muted-foreground">
        {formatTimestamp(effectiveDuration * 1000)}
      </span>
      <div className="flex flex-shrink-0 items-center gap-1 pl-1">
        <button
          type="button"
          disabled={!audioSrc}
          onClick={() => onSeek(Math.max(0, currentTime - 10))}
          title="Back 10 seconds"
          aria-label="Back 10 seconds"
          className="relative flex h-8 w-8 items-center justify-center rounded-lg text-muted-foreground transition-colors hover:bg-[hsl(var(--surface-4))] hover:text-foreground disabled:opacity-45"
        >
          <RotateCcw size={16} />
          <span className="absolute text-[6px] font-bold">10</span>
        </button>
        <button
          type="button"
          disabled={!audioSrc}
          onClick={() => onSeek(Math.min(effectiveDuration, currentTime + 10))}
          title="Forward 10 seconds"
          aria-label="Forward 10 seconds"
          className="relative flex h-8 w-8 items-center justify-center rounded-lg text-muted-foreground transition-colors hover:bg-[hsl(var(--surface-4))] hover:text-foreground disabled:opacity-45"
        >
          <RotateCw size={16} />
          <span className="absolute text-[6px] font-bold">10</span>
        </button>
        <button
          type="button"
          disabled={!audioSrc}
          onClick={onSpeed}
          className="h-8 min-w-[36px] rounded-lg px-1 text-[11.5px] font-bold tabular-nums text-muted-foreground transition-colors hover:bg-[hsl(var(--surface-4))] hover:text-foreground disabled:opacity-45"
          title="Playback speed"
        >
          {speed}x
        </button>
        <button
          type="button"
          disabled={!audioSrc || downloading}
          onClick={onDownload}
          className="flex h-8 w-8 items-center justify-center rounded-lg text-muted-foreground transition-colors hover:bg-[hsl(var(--surface-4))] hover:text-foreground disabled:opacity-45"
          title="Download audio"
          aria-label="Download audio"
        >
          {downloading ? <Loader2 size={14} className="animate-spin" /> : <Download size={14} />}
        </button>
      </div>
    </div>
  );
}

function SpeakerTimeline({
  segments,
  durationMs,
}: {
  segments: MeetingCachedTranscriptSegment[];
  durationMs: number;
}) {
  if (segments.length === 0) return null;
  return (
    <div className="mt-6 flex h-8 w-full overflow-hidden rounded-lg">
      {segments.map((segment, index) => {
        const width = Math.max(0.35, ((Math.max(500, segment.end_ms - segment.start_ms) / Math.max(1, durationMs)) * 100));
        return (
          <button
            type="button"
            key={`${segment.start_ms}-${segment.speaker_id}-${index}`}
            className="h-full min-w-[4px] border-r border-black/20"
            style={{ width: `${width}%`, background: speakerColor(segment.speaker_id) }}
            title={`${formatTimestamp(segment.start_ms)} ${segment.speaker_name}`}
          />
        );
      })}
    </div>
  );
}

function TranscriptTab({
  artifacts,
  onSeekToSegment,
  onCopyTranscript,
  onRetranscribe,
  retranscribing,
}: {
  artifacts: MeetingCachedArtifacts | null;
  onSeekToSegment: (ms: number) => void;
  onCopyTranscript: () => void;
  onRetranscribe: () => void;
  retranscribing: boolean;
}) {
  const segments = artifacts?.segments ?? [];
  const speakers = Array.from(new Set(segments.map((segment) => segment.speaker_id)));
  const durationMs = artifacts?.audio_duration_ms
    ?? segments.reduce((max, segment) => Math.max(max, segment.end_ms), 0)
    ?? 0;
  return (
    <div className="pt-6">
      <div className="flex items-center justify-between gap-3">
        <div className="flex items-center gap-3">
          <h3 className="text-[14px] font-semibold text-foreground">Transcript</h3>
          <span className="text-[12px] font-semibold text-muted-foreground">
            {speakers.length || 0} speaker{speakers.length === 1 ? "" : "s"}
          </span>
        </div>
        <div className="flex items-center gap-2">
          <ToolbarButton
            icon={<RefreshCw size={14} className={retranscribing ? "animate-spin" : ""} />}
            label={retranscribing ? "Re-transcribing…" : "Re-transcribe"}
            disabled={retranscribing}
            onClick={onRetranscribe}
          />
          <CopyButton label="Copy All" disabled={!artifacts?.transcript} onCopy={onCopyTranscript} />
        </div>
      </div>

      <SpeakerTimeline segments={segments} durationMs={durationMs} />

      {segments.length > 0 ? (
        <div className="mt-7 divide-y" style={{ borderColor: "hsl(var(--border))" }}>
          {segments.map((segment, index) => (
            <button
              type="button"
              key={`${segment.start_ms}-${segment.speaker_id}-${index}`}
              onClick={() => onSeekToSegment(segment.start_ms)}
              className="grid w-full grid-cols-[64px_minmax(0,1fr)] gap-5 py-4 text-left hover:brightness-125"
            >
              <span className="pt-0.5 text-[12px] font-bold tabular-nums text-muted-foreground">
                {formatTimestamp(segment.start_ms)}
              </span>
              <span className="min-w-0">
                <span
                  className="text-[13px] font-bold"
                  style={{ color: speakerColor(segment.speaker_id) }}
                >
                  {segment.speaker_name}
                </span>
                <span className="ml-1 inline-flex align-middle text-muted-foreground">
                  <ChevronDown size={13} />
                </span>
                <span className="mt-1 block max-w-[92ch] text-[13.5px] leading-relaxed text-muted-foreground">
                  {segment.text}
                </span>
              </span>
            </button>
          ))}
        </div>
      ) : artifacts?.transcript ? (
        <pre className="mt-6 whitespace-pre-wrap text-[13.5px] leading-relaxed text-muted-foreground">
          {artifacts.transcript}
        </pre>
      ) : (
        <p className="mt-6 text-[13.5px] text-muted-foreground">Transcript is not attached to this meeting yet.</p>
      )}
    </div>
  );
}

function ActionRows({
  meetingAi,
  completed,
  onToggle,
  manualActions,
  actionDraft,
  onActionDraftChange,
  onAddManualAction,
  onToggleManualAction,
  onRemoveManualAction,
}: {
  meetingAi: MeetingIntelligenceResult | null;
  completed: Set<string>;
  onToggle: (key: string) => void;
  manualActions: ManualAction[];
  actionDraft: string;
  onActionDraftChange: (value: string) => void;
  onAddManualAction: () => void;
  onToggleManualAction: (index: number) => void;
  onRemoveManualAction: (index: number) => void;
}) {
  const actions = meetingAi?.action_items ?? [];
  const decisions = meetingAi?.decisions ?? [];
  const actionCount = actions.length + manualActions.length;
  const totalCount = actionCount + decisions.length;
  return (
    <div className="pt-6">
      <div className="mb-7">
        <h3 className="text-[14px] font-semibold text-foreground">Actions & Decisions {totalCount}</h3>
      </div>

      {/* Add a manual action */}
      <div className="mb-6 flex gap-2">
        <input
          value={actionDraft}
          onChange={(event) => onActionDraftChange(event.currentTarget.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              onAddManualAction();
            }
          }}
          placeholder="Add an action item…"
          maxLength={300}
          className="h-10 min-w-0 flex-1 rounded-lg bg-transparent px-3.5 text-[14px] outline-none"
          style={{ border: "1px solid hsl(var(--border))", color: "hsl(var(--foreground))" }}
        />
        <button
          type="button"
          onClick={onAddManualAction}
          disabled={!actionDraft.trim()}
          className="flex h-10 items-center gap-1.5 rounded-lg px-3.5 text-[12px] font-bold disabled:opacity-45"
          style={{ background: "hsl(var(--primary))", color: "hsl(var(--primary-foreground))" }}
        >
          <Plus size={14} /> Add
        </button>
      </div>

      {actionCount > 0 && decisions.length > 0 ? (
        <h4 className="mb-4 text-[11px] font-bold uppercase tracking-[0.18em] text-muted-foreground">
          Actions {actionCount}
        </h4>
      ) : null}

      {manualActions.length > 0 ? (
        <div className="mb-7 space-y-3">
          {manualActions.map((action, index) => (
            <div key={`manual-${index}-${action.title}`} className="group flex items-start gap-3">
              <button
                type="button"
                onClick={() => onToggleManualAction(index)}
                className="mt-0.5 flex h-[18px] w-[18px] flex-shrink-0 items-center justify-center rounded-[4px]"
                style={{
                  background: action.done ? "hsl(var(--primary))" : "transparent",
                  border: action.done ? "1px solid hsl(var(--primary))" : "1px solid hsl(var(--border))",
                  color: "hsl(var(--primary-foreground))",
                }}
                title={action.done ? "Mark incomplete" : "Mark complete"}
              >
                {action.done ? <Check size={12} /> : null}
              </button>
              <p
                className="min-w-0 flex-1 text-[15px] font-semibold"
                style={{
                  color: action.done ? "hsl(var(--muted-foreground))" : "hsl(var(--foreground))",
                  textDecoration: action.done ? "line-through" : "none",
                }}
              >
                {action.title}
                <span className="ml-2 rounded px-1.5 py-0.5 text-[10px] font-bold uppercase tracking-wide" style={{ background: "hsl(var(--surface-3))", color: "hsl(var(--muted-foreground))" }}>
                  added
                </span>
              </p>
              <button
                type="button"
                onClick={() => onRemoveManualAction(index)}
                className="mt-0.5 flex-shrink-0 text-muted-foreground opacity-0 transition-opacity hover:text-foreground group-hover:opacity-100"
                title="Remove action"
              >
                <X size={14} />
              </button>
            </div>
          ))}
        </div>
      ) : null}

      {actions.length === 0 ? (
        actionCount === 0 && decisions.length === 0 ? (
          <p className="text-[14px] text-muted-foreground">
            No action items or decisions found yet. Add your own action above.
          </p>
        ) : null
      ) : (
        <div className={decisions.length > 0 ? "mb-6 space-y-4" : "space-y-4"}>
          {actions.map((action, index) => {
            const key = `${index}-${action.title}`;
            const isDone = completed.has(key);
            return (
              <div key={key} className="grid grid-cols-[22px_minmax(0,1fr)] gap-3">
                <button
                  type="button"
                  onClick={() => onToggle(key)}
                  className="mt-0.5 flex h-[18px] w-[18px] items-center justify-center rounded-[5px]"
                  style={{
                    background: isDone ? "hsl(var(--accent-violet))" : "transparent",
                    border: isDone ? "1px solid hsl(var(--accent-violet))" : "1px solid hsl(var(--border))",
                    color: "hsl(var(--primary-foreground))",
                  }}
                  title={isDone ? "Mark incomplete" : "Mark complete"}
                >
                  {isDone ? <Check size={12} /> : null}
                </button>
                <div className="min-w-0">
                  <p className={`text-[14px] font-semibold ${isDone ? "text-muted-foreground line-through" : "text-foreground"}`}>{action.title}</p>
                  <p className="mt-1 max-w-[92ch] text-[13px] leading-6 text-muted-foreground">
                    {action.evidence || [action.assignee, action.due].filter(Boolean).join(" · ") || "No extra detail captured."}
                  </p>
                </div>
              </div>
            );
          })}
        </div>
      )}

      {decisions.length > 0 ? (
        <section className="space-y-4">
          <h4 className="text-[11px] font-bold uppercase tracking-[0.18em] text-muted-foreground">
            Decisions {decisions.length}
          </h4>
          <div className="space-y-4">
            {decisions.map((decision, index) => (
              <div key={`${decision.text}-${index}`} className="grid grid-cols-[26px_minmax(0,1fr)] gap-4">
                <span
                  className="mt-0.5 flex h-[18px] w-[18px] items-center justify-center rounded-full text-[11px] font-bold"
                  style={{
                    background: "hsl(var(--accent-violet) / 0.14)",
                    color: "hsl(var(--accent-violet))",
                  }}
                >
                  {index + 1}
                </span>
                <div className="min-w-0">
                  <p className="text-[14px] font-semibold text-foreground">{decision.text}</p>
                  {decision.evidence ? (
                    <p className="mt-1 max-w-[108ch] text-[13px] leading-6 text-muted-foreground">{decision.evidence}</p>
                  ) : null}
                </div>
              </div>
            ))}
          </div>
        </section>
      ) : null}
    </div>
  );
}

function DecisionsBlock({ decisions }: { decisions: MeetingAiDecision[] }) {
  if (decisions.length === 0) return null;
  return (
    <section className="mt-7">
      <h3 className="mb-3 text-[14px] font-semibold text-foreground">Decisions</h3>
      <div className="space-y-2.5">
        {decisions.map((decision, index) => (
          <div key={`${decision.text}-${index}`} className="flex gap-2.5 text-[13.5px] leading-6 text-muted-foreground">
            <span className="mt-0.5 flex-shrink-0 text-[11px]" style={{ color: "hsl(var(--accent-violet))" }}>◆</span>
            <p>
              <span className="font-semibold text-foreground">{decision.text}</span>
              {decision.evidence ? <span className="mt-0.5 block text-[12.5px] text-muted-foreground">{decision.evidence}</span> : null}
            </p>
          </div>
        ))}
      </div>
    </section>
  );
}

interface MeetingProcessingStatus {
  meeting_id: string;
  phase: string;
  stage: string;
  running: boolean;
  queued: boolean;
  cancelling: boolean;
  can_cancel: boolean;
  can_retry: boolean;
  error: string | null;
  progress: MeetingProcessingProgress | null;
  has_transcript: boolean;
  has_intelligence: boolean;
  summary_failed: boolean;
  updated_at_ms: number;
}

interface MeetingProcessingProgress {
  stage: string;
  current: number;
  total: number;
  label: string;
  track: string | null;
}

interface ProcessingStartWarning {
  meetingId: string;
  title: string;
  stage: string;
  queued: boolean;
}

// Ordered processing stages shown in the post-meeting progress stepper.
const PROCESSING_STEPS: { key: string; label: string }[] = [
  { key: "queued", label: "Queued" },
  { key: "transcribing", label: "Transcribing" },
  { key: "cleaning", label: "Cleaning" },
  { key: "summarizing", label: "Summarizing" },
  { key: "summarized", label: "Done" },
];

function wait(ms: number): Promise<void> {
  return new Promise((resolve) => window.setTimeout(resolve, ms));
}

function waitForPaint(): Promise<void> {
  return new Promise((resolve) => window.requestAnimationFrame(() => resolve()));
}

function processingStepIndex(stage: string): number {
  const normalized = normalizeProcessingStage(stage);
  const i = PROCESSING_STEPS.findIndex((s) => s.key === normalized);
  if (i >= 0) return i;
  return PROCESSING_STEPS.findIndex((s) => s.key === "transcribing");
}

function effectiveProcessingStage(status: MeetingProcessingStatus): string {
  if (status.running) {
    if (status.cancelling) return "cancelling";
    return normalizeProcessingStage(status.stage || status.phase);
  }
  if (status.phase === "summarized" || status.has_intelligence) return "summarized";
  if (status.summary_failed) return "summarizing";
  if (status.stage === "queued" && status.has_transcript) return "summarizing";
  return normalizeProcessingStage(status.stage || status.phase);
}

function normalizeProcessingStage(stage: string): string {
  switch (stage) {
    case "queued":
    case "transcribing":
    case "cleaning":
    case "summarizing":
    case "summarized":
      return stage;
    case "diarizing":
    case "final_diarizing":
    case "completed":
      return "summarizing";
    case "running":
    case "resuming":
      return "transcribing";
    case "cancelling":
    case "cancelled":
      return "transcribing";
    case "transcribed":
    case "summary_failed":
      return "summarizing";
    case "done":
      return "summarized";
    default:
      return "transcribing";
  }
}

function processingStageLabel(stage: string): string {
  const normalized = normalizeProcessingStage(stage);
  return PROCESSING_STEPS.find((step) => step.key === normalized)?.label ?? "Processing";
}

function processingStepLabel(step: { key: string; label: string }, status: MeetingProcessingStatus, effectiveStage: string): string {
  const progress = status.progress;
  if (
    step.key === "transcribing"
    && normalizeProcessingStage(effectiveStage) === "transcribing"
    && progress?.stage === "transcribing"
    && progress.total > 1
    && progress.current >= 1
  ) {
    return progress.label || `Transcribing ${progress.current}/${progress.total}`;
  }
  return step.label;
}

function isDismissibleProcessingStatus(status: MeetingProcessingStatus): boolean {
  // Only a terminal state is dismissible — a still-processing meeting keeps its
  // live banner (don't let a transient can_retry/error make it dismissible).
  return (
    !status.running &&
    (status.phase === "failed" ||
      status.phase === "cancelled" ||
      status.summary_failed)
  );
}

/** Post-meeting progress banner: a live stage stepper while a background job
 *  runs, or a failure state with a Retry button. Shown above the detail tabs. */
function ProcessingBanner({
  status,
  onRetryTranscribe,
  onRetrySummary,
  onCancel,
  onDismiss,
  retrying,
}: {
  status: MeetingProcessingStatus;
  onRetryTranscribe: () => void;
  onRetrySummary: () => void;
  onCancel: () => void;
  onDismiss: () => void;
  retrying: boolean;
}) {
  const summaryFailed = !status.running && status.summary_failed;
  const cancelled = !status.running && status.phase === "cancelled";
  // "Processing failed" ONLY for a genuine terminal failure — NOT a transient
  // `can_retry`/`error` while the meeting is still recording/transcribing/
  // summarizing (those are processing, not failed). This was the bug: ending a
  // meeting flashed "Processing failed" while the job ran in the background.
  const failed =
    !status.running && (status.phase === "failed" || summaryFailed);
  const onRetry = summaryFailed ? onRetrySummary : onRetryTranscribe;
  const effectiveStage = effectiveProcessingStage(status);
  const activeIdx = processingStepIndex(effectiveStage);
  const trulyQueued = status.queued && effectiveStage === "queued";
  const cancelling = status.running && status.cancelling;
  return (
    <div
      className="mt-5 rounded-xl px-5 py-4"
      style={{
        background: failed ? "hsl(354 60% 12%)" : "hsl(var(--primary) / 0.10)",
        border: `1px solid ${failed ? "hsl(354 56% 32%)" : "hsl(var(--primary) / 0.28)"}`,
      }}
    >
      <div className="flex items-center justify-between gap-3">
        <div className="flex min-w-0 items-center gap-2">
          {failed ? (
            <AlertTriangle size={16} style={{ color: "hsl(354 85% 75%)" }} />
          ) : cancelled ? (
            <Ban size={16} style={{ color: "hsl(var(--muted-foreground))" }} />
          ) : (
            <Loader2 size={16} className="animate-spin" style={{ color: "hsl(var(--primary))" }} />
          )}
          <span className="text-[13px] font-bold text-foreground">
            {summaryFailed
              ? "Summary failed — transcript is saved"
              : cancelled
                ? "Processing cancelled"
              : failed
                ? "Processing failed"
                : cancelling
                  ? "Cancelling processing…"
                  : trulyQueued
                    ? "Queued for processing…"
                    : "Processing your meeting…"}
          </span>
        </div>
        {failed ? (
          <div className="flex shrink-0 items-center gap-2">
            <button
              type="button"
              onClick={onRetry}
              disabled={retrying}
              className="flex h-8 items-center gap-1.5 rounded-lg px-3 text-[12px] font-bold disabled:opacity-50"
              style={{ background: "hsl(var(--surface-3))", color: "hsl(var(--foreground))" }}
            >
              <RefreshCw size={13} className={retrying ? "animate-spin" : ""} />
              {retrying ? "Retrying…" : summaryFailed ? "Regenerate summary" : "Retry"}
            </button>
            <button
              type="button"
              title="Dismiss"
              aria-label="Dismiss processing message"
              onClick={onDismiss}
              className="flex h-8 w-8 items-center justify-center rounded-lg transition-colors hover:text-foreground"
              style={{ background: "hsl(var(--surface-3))", color: "hsl(var(--muted-foreground))" }}
            >
              <X size={14} />
            </button>
          </div>
        ) : cancelled ? (
          <button
            type="button"
            title="Dismiss"
            aria-label="Dismiss processing message"
            onClick={onDismiss}
            className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg transition-colors hover:text-foreground"
            style={{ background: "hsl(var(--surface-3))", color: "hsl(var(--muted-foreground))" }}
          >
            <X size={14} />
          </button>
        ) : status.can_cancel ? (
          <button
            type="button"
            onClick={onCancel}
            className="flex h-8 items-center gap-1.5 rounded-lg px-3 text-[12px] font-bold"
            style={{ background: "hsl(var(--surface-3))", color: "hsl(var(--foreground))" }}
          >
            <X size={13} />
            Cancel
          </button>
        ) : null}
      </div>

      {failed ? (
        status.error ? (
          <p className="mt-2 text-[12px]" style={{ color: "hsl(354 85% 75%)" }}>
            {status.error}
          </p>
        ) : null
      ) : (
        <div className="mt-3 flex flex-wrap items-center gap-1.5">
          {PROCESSING_STEPS.map((step, i) => {
            const done = i < activeIdx;
            const active = i === activeIdx;
            return (
              <div key={step.key} className="flex items-center gap-1.5">
                <span
                  className="text-[11px] font-semibold"
                  style={{
                    color: active
                      ? "hsl(var(--primary))"
                      : done
                        ? "hsl(var(--foreground))"
                        : "hsl(var(--muted-foreground))",
                  }}
                >
                  {processingStepLabel(step, status, effectiveStage)}
                </span>
                {i < PROCESSING_STEPS.length - 1 ? (
                  <span style={{ color: "hsl(var(--muted-foreground))" }}>›</span>
                ) : null}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

interface MeetingsViewProps {
  onJoinMeeting?: (meetingId: string) => void;
  /** A meeting to auto-select when the view mounts/updates (e.g. the one that
   *  just ended). Consumed once via onFocusConsumed. */
  focusMeetingId?: string | null;
  onFocusConsumed?: () => void;
  /** Open Settings → Enterprise (to select/activate a workspace). */
  onOpenWorkspaces?: () => void;
}

// The one and only meeting transcription model: Oriserve Hinglish,
// shared with dictation. There is no model picker; first run downloads this and
// nothing else. Renamed in one place via `@/lib/onDeviceModel`.
const MEETING_MODEL_NAME = NEW_MODEL_FILE;

export function MeetingsView({
  onJoinMeeting,
  focusMeetingId,
  onFocusConsumed,
  onOpenWorkspaces,
}: MeetingsViewProps) {
  const [meetings, setMeetings] = useState<Meeting[]>([]);
  const [loading, setLoading] = useState(true);
  // True once the list has loaded successfully at least once. After that we never
  // show the full-screen "Loading…" spinner again — refreshes happen in the
  // background and the last-known list stays visible — so a slow/orphaned poll
  // can never wedge the view on the spinner.
  const hasLoadedRef = useRef(false);
  const [error, setError] = useState("");
  const [creating, setCreating] = useState(false);
  const [clearing, setClearing] = useState(false);
  const [meetingInProgress, setMeetingInProgress] = useState(false);
  const [activeMeetingId, setActiveMeetingId] = useState<string | null>(null);
  const [dateFilter, setDateFilter] = useState<MeetingListFilter>("all");
  const [activeTag, setActiveTag] = useState<string | null>(null);
  const [searchQuery, setSearchQuery] = useState("");
  const [searchHits, setSearchHits] = useState<Record<string, MeetingSearchHit> | null>(null);
  const [searchBusy, setSearchBusy] = useState(false);
  const [selectedMeetingId, setSelectedMeetingId] = useState<string | null>(null);
  const [detailTab, setDetailTab] = useState<DetailTab>("summary");
  const [procStatus, setProcStatus] = useState<MeetingProcessingStatus | null>(null);
  const dismissedProcessingIdsRef = useRef<Set<string>>(new Set());
  const [meetingAi, setMeetingAi] = useState<MeetingIntelligenceResult | null>(null);
  const [meetingAiLoading, setMeetingAiLoading] = useState(false);
  const [meetingAiError, setMeetingAiError] = useState<string | null>(null);
  const [artifacts, setArtifacts] = useState<MeetingCachedArtifacts | null>(null);
  const [artifactsLoading, setArtifactsLoading] = useState(false);
  const [completedActions, setCompletedActions] = useState<Set<string>>(new Set());
  const [manualActions, setManualActions] = useState<ManualAction[]>([]);
  const [actionDraft, setActionDraft] = useState("");
  const [overviews, setOverviews] = useState<Record<string, MeetingOverview>>({});
  const [userTags, setUserTags] = useState<string[]>([]);
  const [addingTag, setAddingTag] = useState(false);
  const [tagDraft, setTagDraft] = useState("");
  const [notes, setNotes] = useState("");
  const [notesStatus, setNotesStatus] = useState<"idle" | "saving" | "saved">("idle");
  const [notesPreview, setNotesPreview] = useState(true);
  const [retranscribing, setRetranscribing] = useState(false);
  const [editingTitle, setEditingTitle] = useState(false);
  const [titleDraft, setTitleDraft] = useState("");
  const [pendingDelete, setPendingDelete] = useState<{ id: string; title: string } | null>(null);
  const [processingStartWarning, setProcessingStartWarning] =
    useState<ProcessingStartWarning | null>(null);
  const [audioCurrentTime, setAudioCurrentTime] = useState(0);
  const [audioDuration, setAudioDuration] = useState(0);
  const [audioPlaying, setAudioPlaying] = useState(false);
  const [audioRate, setAudioRate] = useState(1);
  const audioRef = useRef<HTMLAudioElement | null>(null);

  const fetchMeetings = useCallback(async (opts?: { background?: boolean }) => {
    // Background refreshes (interval polls) never toggle the spinner — they
    // update the list in place so the view never flashes "Loading…" after the
    // first successful load.
    const background = opts?.background ?? false;
    if (!background) setLoading(true);
    setError("");
    try {
      // Meetings are local-only: the list is the set of meeting folders on this
      // device, enumerated by the engine (no control-plane).
      //
      // Robustness: a Tauri `invoke` promise can occasionally never settle — e.g.
      // if the webview reloads while the command's response is in flight, the
      // callback is orphaned ("Couldn't find callback id"). Without a bound, that
      // wedges the list on "Loading meetings…" forever. Race the call against a
      // timeout so the spinner always clears; the regular 15s poll then retries,
      // and any already-loaded meetings stay on screen (we only replace on success).
      const local = await withTimeout(
        invoke<LocalMeetingSummary[]>("meeting_engine_list_meetings"),
        8000,
        "list_meetings timed out",
      );
      setMeetings(local.map(localToMeeting));
      const map: Record<string, MeetingOverview> = {};
      for (const m of local) map[m.id] = localToOverview(m);
      setOverviews(map);
      hasLoadedRef.current = true;
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load meetings");
    } finally {
      if (!background) setLoading(false);
    }
  }, []);

  // Re-read the local list after a mutation (rename/favorite/hide) so the cards
  // reflect the updated overrides registry, without the loading flash.
  const refreshOverviews = useCallback(async () => {
    try {
      const local = await invoke<LocalMeetingSummary[]>("meeting_engine_list_meetings");
      const map: Record<string, MeetingOverview> = {};
      for (const m of local) map[m.id] = localToOverview(m);
      setOverviews(map);
    } catch {
      /* best-effort */
    }
  }, []);

  useEffect(() => {
    void fetchMeetings();
    const interval = setInterval(() => void fetchMeetings({ background: true }), 15_000);
    return () => clearInterval(interval);
  }, [fetchMeetings]);

  // A meeting can't be transcribed without an installed model. Poll the installed
  // model list (null = still checking) so we can block starting + prompt to
  // download. 30s (was 5s): this fires TWO ipc:// invokes per tick and the model
  // set only changes on a download — at 5s it was a needless, steady drain on the
  // ~6-connection WebView2 pool (each invoke also costs a CORS preflight). The
  // download flow refreshes this explicitly, so 30s is plenty to clear the banner.
  const [hasModel, setHasModel] = useState<boolean | null>(null);
  const [downloadingModel, setDownloadingModel] = useState(false);
  const refreshHasModel = useCallback(async () => {
    try {
      // Keep a model selected whenever one is installed (auto-select single).
      await invoke("meeting_ensure_active_model").catch(() => null);
      const models = await invoke<{ incomplete: boolean }[]>("meeting_list_whisper_models");
      setHasModel(models.some((m) => !m.incomplete));
    } catch {
      setHasModel(null);
    }
  }, []);
  useEffect(() => {
    void refreshHasModel();
    const id = setInterval(() => void refreshHasModel(), 30_000);
    return () => clearInterval(id);
  }, [refreshHasModel]);

  // First-run provisioning: there is no model picker, so just fetch Oriserve.
  const downloadMeetingModel = useCallback(async () => {
    if (downloadingModel) return;
    setDownloadingModel(true);
    setError("");
    try {
      await invoke("meeting_download_whisper_model", { name: MEETING_MODEL_NAME });
      await refreshHasModel();
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      if (msg !== "cancelled") setError(`Couldn't download the transcription model: ${msg}`);
    } finally {
      setDownloadingModel(false);
    }
  }, [downloadingModel, refreshHasModel]);

  const startNewLocalMeeting = useCallback(async () => {
    setCreating(true);
    setError("");
    try {
      // Local-only: allocate a fresh on-device meeting id and open the recorder.
      // No cloud record is created, so an abandoned meeting leaves nothing behind.
      const id = await invoke<string>("meeting_engine_new_local_meeting");
      onJoinMeeting?.(id);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to create meeting");
    } finally {
      setCreating(false);
    }
  }, [onJoinMeeting]);

  const findRunningProcessingMeeting = useCallback(async (): Promise<ProcessingStartWarning | null> => {
    for (const meeting of meetings) {
      try {
        const status = await invoke<MeetingProcessingStatus>("meeting_engine_get_processing_status", {
          meetingId: meeting.id,
        });
        if (!status.running) continue;
        const title = overviews[meeting.id]?.title?.trim() || meeting.title || "another meeting";
        return {
          meetingId: meeting.id,
          title,
          stage: effectiveProcessingStage(status),
          queued: status.queued,
        };
      } catch {
        // Best-effort warning. If a single old folder has bad status, keep checking
        // others and let the engine handle the final start decision.
      }
    }
    return null;
  }, [meetings, overviews]);

  const handleNewMeeting = useCallback(async () => {
    // Screen Recording permission is required to capture meeting system audio
    // (ScreenCaptureKit). Check it FIRST and prompt if missing — never start
    // capture and surprise the user with the macOS dialog mid-meeting. (No-op on
    // platforms without this permission; the command returns true there.)
    try {
      const granted = await invoke<boolean>("screen_recording_granted");
      if (!granted) {
        // Raises the macOS prompt the first time and opens the Screen Recording
        // pane; macOS usually only honors a fresh grant after a relaunch.
        await invoke<boolean>("request_screen_recording");
        setError(
          "AirNote needs Screen Recording permission to capture meeting audio. Enable it in System Settings → Privacy & Security → Screen Recording, then reopen AirNote and try again.",
        );
        return;
      }
    } catch {
      /* permission probe failed — fall through; capture surfaces its own error */
    }

    if (hasModel === false) {
      void downloadMeetingModel();
      return;
    }
    // Never start a second meeting while one is already recording — show a popup
    // (with a jump-to-it action) instead of silently stopping the first.
    try {
      const status = await invoke<{ active: boolean; session_id?: string | null }>(
        "meeting_engine_get_status",
      );
      if (status.active) {
        setActiveMeetingId(status.session_id ?? null);
        setMeetingInProgress(true);
        return;
      }
    } catch {
      /* status check failed — fall through and let the engine handle it */
    }

    const runningProcessing = await findRunningProcessingMeeting();
    if (runningProcessing) {
      setProcessingStartWarning(runningProcessing);
      return;
    }

    await startNewLocalMeeting();
  }, [downloadMeetingModel, findRunningProcessingMeeting, hasModel, startNewLocalMeeting]);

  const handlePauseProcessingAndStart = useCallback(async () => {
    const warning = processingStartWarning;
    if (!warning || creating) return;

    setCreating(true);
    setError("");
    try {
      await invoke("meeting_engine_cancel_processing", { meetingId: warning.meetingId });
      setProcessingStartWarning(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to pause current processing");
      setCreating(false);
      return;
    }

    await startNewLocalMeeting();
  }, [creating, processingStartWarning, startNewLocalMeeting]);

  // Clear empty meetings — silence/noise recordings (few words, never analyzed,
  // not favorited or renamed) that pile up. Analyzed/favorited/named meetings and
  // the active recording are always kept.
  const handleClearEmpty = useCallback(async () => {
    setClearing(true);
    try {
      await invoke<number>("meeting_engine_clear_empty_meetings");
      await fetchMeetings();
    } catch (err) {
      console.warn("[meeting] clear empty failed:", err);
    } finally {
      setClearing(false);
    }
  }, [fetchMeetings]);

  // Debounced full-text search across title/tags/summary/notes/decisions/
  // actions/transcript (heavy fields are read locally by the backend).
  useEffect(() => {
    const query = searchQuery.trim();
    if (!query) {
      setSearchHits(null);
      setSearchBusy(false);
      return;
    }
    setSearchBusy(true);
    let cancelled = false;
    const handle = window.setTimeout(() => {
      invoke<MeetingSearchHit[]>("meeting_engine_search_meetings", {
        query,
        meetings: meetings.map((meeting) => ({ id: meeting.id, title: meeting.title })),
      })
        .then((hits) => {
          if (cancelled) return;
          const map: Record<string, MeetingSearchHit> = {};
          for (const hit of hits) map[hit.id] = hit;
          setSearchHits(map);
        })
        .catch(() => {
          if (!cancelled) setSearchHits({});
        })
        .finally(() => {
          if (!cancelled) setSearchBusy(false);
        });
    }, 220);
    return () => {
      cancelled = true;
      window.clearTimeout(handle);
    };
  }, [searchQuery, meetings]);

  const searching = searchQuery.trim().length > 0;
  const filteredMeetings = meetings.filter((meeting) => {
    const ov = overviews[meeting.id];
    // Recent-tag filter (AND with every other constraint below).
    if (activeTag && !(ov?.tags ?? []).includes(activeTag)) return false;
    // Archived tab: ONLY meetings removed from the list whose files are still on
    // disk (not file-deleted). Other tabs exclude archived meetings.
    if (dateFilter === "archived") {
      if (searching) return Boolean(ov?.hidden && ov?.has_local_files && searchHits?.[meeting.id]);
      return Boolean(ov?.hidden && ov?.has_local_files);
    }
    if (ov?.hidden) return false;
    if (dateFilter === "favorites" && !ov?.favorite) return false;
    // When searching, restrict to backend hits. Date tabs are ignored so older
    // matches are still findable; Favorites stays scoped to favorite meetings.
    if (searching) return Boolean(searchHits?.[meeting.id]);
    if (dateFilter === "favorites") return true;
    const time = meetingTime(meeting);
    if (Number.isNaN(time.getTime())) return dateFilter === "all";
    if (dateFilter === "today") return isSameLocalDay(time, new Date());
    if (dateFilter === "week") return isInCurrentWeek(time);
    return true;
  });

  const sortedMeetings = [...filteredMeetings].sort((a, b) => {
    // Search results rank by relevance score; otherwise live-first then recent.
    if (searching) {
      return (searchHits?.[b.id]?.score ?? 0) - (searchHits?.[a.id]?.score ?? 0);
    }
    const order: Record<string, number> = { live: 0, scheduled: 1, ended: 2 };
    const statusDiff = (order[a.status] ?? 2) - (order[b.status] ?? 2);
    if (statusDiff !== 0) return statusDiff;
    return meetingTime(b).getTime() - meetingTime(a).getTime();
  });

  const selectedMeeting = selectedMeetingId
    ? sortedMeetings.find((meeting) => meeting.id === selectedMeetingId) ?? null
    : null;

  // The detail is a modal now: it opens only on an explicit click, never
  // auto-selects the first meeting. But if the open meeting gets filtered out,
  // hidden, or deleted, close the modal so state stays consistent.
  useEffect(() => {
    if (selectedMeetingId === null) return;
    const stillVisible = sortedMeetings.some((meeting) => meeting.id === selectedMeetingId);
    if (!stillVisible) setSelectedMeetingId(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedMeetingId, sortedMeetings.length]);

  // Escape closes the meeting modal — unless an inline title/tag editor is
  // capturing the key (it uses Escape to cancel its own edit).
  useEffect(() => {
    if (!selectedMeetingId) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !editingTitle && !addingTag) setSelectedMeetingId(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [selectedMeetingId, editingTitle, addingTag]);

  // Most-recent distinct tags across meetings, for the quick-filter row.
  const recentTags = (() => {
    const seen = new Set<string>();
    const ordered: string[] = [];
    for (const meeting of sortedMeetings) {
      for (const tag of overviews[meeting.id]?.tags ?? []) {
        if (seen.has(tag)) continue;
        seen.add(tag);
        ordered.push(tag);
        if (ordered.length >= 12) break;
      }
      if (ordered.length >= 12) break;
    }
    return ordered;
  })();

  // Honor an externally-requested focus (e.g. the meeting that just ended).
  // Select it as soon as it appears in the list, then clear the request so the
  // user can navigate freely afterward.
  useEffect(() => {
    if (!focusMeetingId) return;
    setSelectedMeetingId(focusMeetingId);
    setDetailTab("summary");
    // Refresh the list so the just-created/ended meeting is present.
    void fetchMeetings();
    onFocusConsumed?.();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [focusMeetingId]);

  const liveCount = meetings.filter((meeting) => meeting.status === "live").length;
  const endedCount = meetings.filter((meeting) => meeting.status === "ended").length;
  const selectedActionCount = meetingAi?.action_items?.length ?? 0;
  const selectedDecisionCount = meetingAi?.decisions?.length ?? 0;
  const selectedActionDecisionCount = selectedActionCount + selectedDecisionCount + manualActions.length;
  const transcriptWordCount = wordCount(artifacts?.transcript);
  const selectedWordCount = transcriptWordCount || wordCount(meetingAi?.summary) || wordCount(selectedMeeting?.agenda);
  const sectionLabel =
    dateFilter === "favorites"
      ? "FAVORITES"
      : dateFilter === "archived"
        ? "ARCHIVED"
        : sortedMeetings[0]
          ? meetingTime(sortedMeetings[0]).toLocaleDateString(undefined, { month: "short", year: "numeric" }).toUpperCase()
          : "RECENT";
  const emptyMeetingMessage =
    dateFilter === "favorites"
      ? "No favorite meetings yet"
      : dateFilter === "archived"
        ? "No archived meetings"
        : "No meetings for this filter";

  // NOTE: the sidebar is driven solely by the strict, per-meeting backend
  // overviews (`get_meeting_overviews`). We deliberately do NOT mirror the
  // selected meeting's `meetingAi` into the overviews map — doing so stamped the
  // previously-viewed meeting's data onto the newly-clicked card (effects run
  // before `meetingAi` is cleared). Mutations call `refreshOverviews()` instead.

  const audioSrc = useMemo(() => {
    if (!artifacts?.audio_path) return null;
    return convertFileSrc(artifacts.audio_path);
  }, [artifacts?.audio_path]);

  // AI-generated tags merged with the user's own tags (case-insensitive
  // de-dupe). AI tags are display-only; user tags can be removed.
  const tags = useMemo(() => {
    // AI tags come from the overview (already filtered of dismissed tags by the
    // backend); user tags are appended and removable.
    const aiTags = (selectedMeeting && overviews[selectedMeeting.id]?.tags) || [];
    const seen = new Set<string>();
    const merged: Array<{ label: string; color: string; source: "ai" | "user" }> = [];
    for (const label of aiTags) {
      const key = label.trim().toLowerCase();
      if (!label.trim() || seen.has(key)) continue;
      seen.add(key);
      merged.push({ label: label.trim(), color: tagColor(label), source: "ai" });
    }
    for (const label of userTags) {
      const key = label.trim().toLowerCase();
      if (!label.trim() || seen.has(key)) continue;
      seen.add(key);
      merged.push({ label: label.trim(), color: tagColor(label), source: "user" });
    }
    return merged;
  }, [overviews, selectedMeeting, userTags]);

  useEffect(() => {
    if (!selectedMeeting) {
      setMeetingAi(null);
      setMeetingAiError(null);
      setMeetingAiLoading(false);
      setArtifacts(null);
      setArtifactsLoading(false);
      setUserTags([]);
      return;
    }

    let cancelled = false;
    // Clear the previous meeting's data immediately so its title/summary never
    // lingers on screen while the newly-selected meeting loads.
    setMeetingAi(null);
    setArtifacts(null);
    setMeetingAiLoading(true);
    setMeetingAiError(null);
    setArtifactsLoading(true);
    setCompletedActions(new Set());
    setUserTags([]);
    setAddingTag(false);
    setTagDraft("");
    setNotes("");
    setNotesStatus("idle");
    setNotesPreview(true);
    setManualActions([]);
    setActionDraft("");

    invoke<ManualAction[]>("meeting_engine_get_manual_actions", { meetingId: selectedMeeting.id })
      .then((result) => {
        if (!cancelled) setManualActions(result);
      })
      .catch(() => {
        if (!cancelled) setManualActions([]);
      });

    invoke<string[]>("meeting_engine_get_user_tags", { meetingId: selectedMeeting.id })
      .then((result) => {
        if (!cancelled) setUserTags(result);
      })
      .catch(() => {
        if (!cancelled) setUserTags([]);
      });

    invoke<string>("meeting_engine_get_notes", { meetingId: selectedMeeting.id })
      .then((result) => {
        if (!cancelled) setNotes(result);
      })
      .catch(() => {
        if (!cancelled) setNotes("");
      });

    invoke<MeetingIntelligenceResult | null>("meeting_engine_get_cached_intelligence", { meetingId: selectedMeeting.id })
      .then((result) => {
        if (!cancelled) setMeetingAi(result);
      })
      .catch((err) => {
        if (!cancelled) {
          setMeetingAi(null);
          setMeetingAiError(err instanceof Error ? err.message : String(err));
        }
      })
      .finally(() => {
        if (!cancelled) setMeetingAiLoading(false);
      });

    invoke<MeetingCachedArtifacts | null>("meeting_engine_get_cached_artifacts", { meetingId: selectedMeeting.id })
      .then((result) => {
        if (!cancelled) setArtifacts(result);
      })
      .catch(() => {
        if (!cancelled) setArtifacts(null);
      })
      .finally(() => {
        if (!cancelled) setArtifactsLoading(false);
      });

    return () => {
      cancelled = true;
    };
  }, [selectedMeeting?.id]);

  useEffect(() => {
    setAudioCurrentTime(0);
    setAudioDuration((artifacts?.audio_duration_ms ?? 0) / 1000);
    setAudioPlaying(false);
  }, [audioSrc, artifacts?.audio_duration_ms]);

  useEffect(() => {
    if (audioRef.current) audioRef.current.playbackRate = audioRate;
  }, [audioRate, audioSrc]);

  const toggleAudio = useCallback(() => {
    const audio = audioRef.current;
    if (!audio) return;
    if (audio.paused) {
      void audio.play();
      setAudioPlaying(true);
    } else {
      audio.pause();
      setAudioPlaying(false);
    }
  }, []);

  const seekAudio = useCallback((seconds: number) => {
    const audio = audioRef.current;
    if (!audio) return;
    const next = Math.max(0, seconds);
    audio.currentTime = next;
    setAudioCurrentTime(next);
  }, []);

  const seekToSegment = useCallback((ms: number) => {
    const audio = audioRef.current;
    if (!audio) return;
    audio.currentTime = Math.max(0, ms / 1000);
    setAudioCurrentTime(audio.currentTime);
    void audio.play();
    setAudioPlaying(true);
  }, []);

  const cycleSpeed = useCallback(() => {
    setAudioRate((current) => {
      if (current === 1) return 1.25;
      if (current === 1.25) return 1.5;
      if (current === 1.5) return 2;
      return 1;
    });
  }, []);

  const [downloadingAudio, setDownloadingAudio] = useState(false);
  const handleDownloadAudio = useCallback(async () => {
    const path = artifacts?.audio_path;
    if (!path || downloadingAudio) return;
    setDownloadingAudio(true);
    try {
      const base =
        (selectedMeeting?.title || "meeting")
          .replace(/[^\w-]+/g, "_")
          .replace(/^_+|_+$/g, "")
          .slice(0, 60) || "meeting";
      const ext = path.split(".").pop()?.toLowerCase() || "wav";
      const saved = await invoke<string | null>("download_meeting_audio", {
        audioPath: path,
        filename: `${base}.${ext}`,
      });
      if (saved) await invoke("reveal_downloaded_file", { path: saved }).catch(() => {});
    } catch (err) {
      console.warn("[meeting] audio download failed:", err);
    } finally {
      setDownloadingAudio(false);
    }
  }, [artifacts?.audio_path, selectedMeeting?.title, downloadingAudio]);

  const copySummary = useCallback(async () => {
    const text = meetingAi?.summary?.trim();
    if (!text) return;
    await navigator.clipboard.writeText(text);
  }, [meetingAi?.summary]);

  // Export the locally-generated minutes to a beautifully formatted Lark Docx
  // (server-side) and create Lark tasks for assigned owners. Persists the doc
  // URL on the meeting so "Open in Lark" appears afterwards.
  const [larkExporting, setLarkExporting] = useState(false);
  const [larkError, setLarkError] = useState<string | null>(null);
  // When the failure is an auth/scope problem, we offer a "Reconnect Lark"
  // action instead of a dead-end error.
  const [larkNeedsReauth, setLarkNeedsReauth] = useState(false);
  // Meetings list vs cross-meeting Digest mode.
  const [viewMode, setViewMode] = useState<"meetings" | "digest">("meetings");
  const handleExportToLark = useCallback(async () => {
    if (!selectedMeeting || !meetingAi?.summary?.trim()) return;
    setLarkExporting(true);
    setLarkError(null);
    setLarkNeedsReauth(false);
    try {
      const result = await exportMeetingToLark({
        title: meetingAi.title?.trim() || selectedMeeting.title,
        summary: meetingAi.summary,
        action_items: (meetingAi.action_items ?? []).map((a) => ({
          title: a.title,
          assignee: a.assignee ?? null,
        })),
        decisions: (meetingAi.decisions ?? []).map((d) => d.text).filter(Boolean),
      });
      if (result.ok) {
        const mid = selectedMeeting.id;
        await invoke("meeting_engine_set_meeting_lark_doc", { meetingId: mid, url: result.url }).catch(
          () => {},
        );
        setOverviews((prev) => ({
          ...prev,
          [mid]: { ...(prev[mid] ?? {}), lark_doc_url: result.url },
        }));
        if (result.url) void openExternal(result.url);
      } else {
        setLarkError(result.message);
        // Codes that mean "the Lark login needs refreshing" → offer reconnect.
        setLarkNeedsReauth(
          result.code === "lark_reauth_required" ||
            result.code === "lark_not_linked" ||
            result.code === "unauthorized",
        );
      }
    } finally {
      setLarkExporting(false);
    }
  }, [selectedMeeting, meetingAi]);

  const copyTranscript = useCallback(async () => {
    const text = artifacts?.transcript?.trim();
    if (!text) return;
    await navigator.clipboard.writeText(text);
  }, [artifacts?.transcript]);

  const handleReanalyze = useCallback(async () => {
    if (!selectedMeeting || meetingAiLoading) return;
    setMeetingAiLoading(true);
    setMeetingAiError(null);
    try {
      const result = await invoke<MeetingIntelligenceResult>("meeting_engine_generate_intelligence", {
        meetingId: selectedMeeting.id,
      });
      setMeetingAi(result);
      await refreshOverviews();
    } catch (err) {
      setMeetingAiError(err instanceof Error ? err.message : String(err));
    } finally {
      setMeetingAiLoading(false);
    }
  }, [selectedMeeting, meetingAiLoading, refreshOverviews]);

  const selectedIdRef = useRef<string | null>(null);
  selectedIdRef.current = selectedMeeting?.id ?? null;

  const handleRetranscribe = useCallback(async () => {
    if (!selectedMeeting || retranscribing || procStatus?.running) return;
    const id = selectedMeeting.id;
    const optimisticStatus: MeetingProcessingStatus = {
      meeting_id: id,
      phase: "transcribing",
      stage: "queued",
      running: true,
      queued: true,
      cancelling: false,
      can_cancel: true,
      can_retry: false,
      error: null,
      progress: null,
      has_transcript: Boolean(artifacts?.transcript?.trim()),
      has_intelligence: Boolean(meetingAi?.summary?.trim()),
      summary_failed: false,
      updated_at_ms: Date.now(),
    };
    setRetranscribing(true);
    dismissedProcessingIdsRef.current.delete(id);
    setProcStatus(optimisticStatus);
    procWasRunningRef.current = true;
    try {
      // Let React paint the queued/spinner state before the native side scans WAV
      // metadata and enqueues the background job.
      await waitForPaint();
      await invoke("meeting_engine_retranscribe", { meetingId: id });
      const startedAt = Date.now();
      for (;;) {
        let status: MeetingProcessingStatus;
        try {
          status = await invoke<MeetingProcessingStatus>("meeting_engine_get_processing_status", {
            meetingId: id,
          });
        } catch (statusErr) {
          console.warn("[meeting] processing status poll failed:", statusErr);
          break;
        }
        if (selectedIdRef.current === id) setProcStatus(status);
        if (!status.running || Date.now() - startedAt > 6 * 60 * 1000) break;
        await wait(1500);
      }
      // Reload artifacts, but only if this meeting is still selected.
      const fresh = await invoke<MeetingCachedArtifacts | null>("meeting_engine_get_cached_artifacts", {
        meetingId: id,
      });
      if (selectedIdRef.current === id) setArtifacts(fresh);
      await refreshOverviews();
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      console.warn("[meeting] re-transcribe failed:", err);
      if (selectedIdRef.current === id) {
        setProcStatus({
          meeting_id: id,
          phase: "failed",
          stage: "failed",
          running: false,
          queued: false,
          cancelling: false,
          can_cancel: false,
          can_retry: true,
          error: message,
          progress: null,
          has_transcript: Boolean(artifacts?.transcript?.trim()),
          has_intelligence: Boolean(meetingAi?.summary?.trim()),
          summary_failed: false,
          updated_at_ms: Date.now(),
        });
      }
    } finally {
      setRetranscribing(false);
    }
  }, [
    selectedMeeting,
    retranscribing,
    procStatus?.running,
    artifacts?.transcript,
    meetingAi?.summary,
    refreshOverviews,
  ]);

  const handleDismissProcessingBanner = useCallback(() => {
    const id = procStatus?.meeting_id;
    if (!id) return;
    dismissedProcessingIdsRef.current.add(id);
    setProcStatus((current) => (current?.meeting_id === id ? null : current));
  }, [procStatus?.meeting_id]);

  const handleCancelProcessing = useCallback(async () => {
    const id = selectedMeeting?.id;
    if (!id || !procStatus?.running || procStatus.cancelling) return;
    setProcStatus((current) =>
      current && current.meeting_id === id
        ? { ...current, cancelling: true, can_cancel: false, stage: "cancelling" }
        : current,
    );
    try {
      await invoke("meeting_engine_cancel_processing", { meetingId: id });
      const status = await invoke<MeetingProcessingStatus>("meeting_engine_get_processing_status", {
        meetingId: id,
      });
      if (selectedIdRef.current === id) setProcStatus(status);
    } catch (err) {
      console.warn("[meeting] cancel processing failed:", err);
      if (selectedIdRef.current === id) {
        setProcStatus((current) =>
          current && current.meeting_id === id
            ? {
                ...current,
                cancelling: false,
                can_cancel: current.running,
                error: err instanceof Error ? err.message : String(err),
              }
            : current,
        );
      }
    }
  }, [procStatus?.cancelling, procStatus?.running, selectedMeeting?.id]);

  // Poll per-meeting processing status so the post-meeting stages (transcribing
  // → cleaning → summarizing → ready) render live, and reload artifacts the moment
  // a background job finishes. Polls every 2s only while a job is active, so an
  // idle/finished meeting costs a single call.
  const procWasRunningRef = useRef(false);
  // Grace-window state so the poll keeps refreshing for a bit AFTER a job stops
  // running — the worker finalizes the auto-summary a beat later, and a transient
  // post-transcription status must not latch "Processing failed" until the user
  // switches tabs (which remounts + re-fetches the now-correct state).
  const procSettleTicksRef = useRef(0);
  const procReloadedDoneRef = useRef(false);
  useEffect(() => {
    const id = selectedMeeting?.id;
    if (!id) {
      setProcStatus(null);
      procWasRunningRef.current = false;
      return;
    }
    procSettleTicksRef.current = 0;
    procReloadedDoneRef.current = false;
    let cancelled = false;
    let timer: number | undefined;
    const tick = async () => {
      let status: MeetingProcessingStatus | null = null;
      try {
        status = await invoke<MeetingProcessingStatus>("meeting_engine_get_processing_status", {
          meetingId: id,
        });
      } catch {
        status = null;
      }
      if (cancelled || selectedIdRef.current !== id) return;
      const running = Boolean(status?.running);
      if (running) {
        dismissedProcessingIdsRef.current.delete(id);
      }
      if (
        status &&
        !running &&
        dismissedProcessingIdsRef.current.has(id) &&
        isDismissibleProcessingStatus(status)
      ) {
        setProcStatus(null);
      } else {
        setProcStatus(status);
      }
      const wasRunning = procWasRunningRef.current;
      procWasRunningRef.current = running;

      // The auto-summary finalizes a beat after the job goes not-running, so a
      // single reload on the running→false edge misses it. Reload on that edge
      // AND again the moment the summary actually lands.
      const reachedSummary =
        !!status &&
        !running &&
        (status.has_intelligence === true || status.phase === "summarized");
      const shouldReload =
        (wasRunning && !running) || (reachedSummary && !procReloadedDoneRef.current);
      if (shouldReload) {
        if (reachedSummary) procReloadedDoneRef.current = true;
        try {
          const fresh = await invoke<MeetingCachedArtifacts | null>(
            "meeting_engine_get_cached_artifacts",
            { meetingId: id },
          );
          if (!cancelled && selectedIdRef.current === id) setArtifacts(fresh);
        } catch {
          /* best-effort */
        }
        try {
          const intel = await invoke<MeetingIntelligenceResult | null>(
            "meeting_engine_get_cached_intelligence",
            { meetingId: id },
          );
          if (!cancelled && selectedIdRef.current === id && intel) {
            setMeetingAi(intel);
            setMeetingAiError(null);
          }
        } catch {
          /* best-effort */
        }
        void refreshOverviews();
        void fetchMeetings();
      }

      // Keep polling while running, and for a bounded grace window after it stops,
      // until the meeting truly settles — a finished summary, or a stable terminal
      // phase. This is what prevents a transient post-transcription status from
      // showing "Processing failed" until a tab switch forces a remount.
      // Keep polling while a job runs, while the meeting is in ANY active /
      // processing phase (recording → transcribing → cleaning → summarizing, so
      // the banner tracks the whole post-End pipeline and the poll never stops
      // mid-flight), and briefly after to catch the final summary. Stop only at a
      // terminal state (summarized / failed / cancelled). This is what prevents a
      // stale "Processing failed" from showing after End until you switch meetings.
      const phase = status?.phase ?? "";
      const terminal =
        phase === "summarized" || phase === "failed" || phase === "cancelled";
      const activePhase =
        running ||
        [
          "recording",
          "transcribing",
          "cleaning",
          "summarizing",
          "queued",
          "diarizing",
          "final_diarizing",
        ].includes(phase);
      if (activePhase) {
        procSettleTicksRef.current = 0;
      } else if (!terminal) {
        // "transcribed" awaiting summary, or the brief post-End enqueue gap.
        procSettleTicksRef.current += 1;
      }
      const keepPolling =
        activePhase || (!terminal && procSettleTicksRef.current <= 10);
      if (keepPolling && !cancelled) {
        timer = window.setTimeout(() => void tick(), 2000);
      }
    };
    void tick();
    return () => {
      cancelled = true;
      if (timer) window.clearTimeout(timer);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedMeeting?.id]);

  const seekToSeconds = useCallback(
    (seconds: number) => seekToSegment(Math.max(0, seconds) * 1000),
    [seekToSegment],
  );

  // Debounced notes autosave. Captures the meeting id + value so a save still
  // lands correctly even if the user switches meetings before it fires.
  const notesSaveTimer = useRef<number | null>(null);
  const handleNotesChange = useCallback(
    (value: string) => {
      setNotes(value);
      const meeting = selectedMeeting;
      if (!meeting) return;
      setNotesStatus("saving");
      if (notesSaveTimer.current) window.clearTimeout(notesSaveTimer.current);
      notesSaveTimer.current = window.setTimeout(() => {
        invoke("meeting_engine_set_notes", { meetingId: meeting.id, notes: value })
          .then(() => setNotesStatus("saved"))
          .catch(() => setNotesStatus("idle"));
      }, 600);
    },
    [selectedMeeting],
  );

  const persistManualActions = useCallback(
    (next: ManualAction[]) => {
      setManualActions(next);
      if (!selectedMeeting) return;
      void invoke("meeting_engine_set_manual_actions", {
        meetingId: selectedMeeting.id,
        items: next,
      }).catch(() => {});
    },
    [selectedMeeting],
  );

  const handleAddManualAction = useCallback(() => {
    const title = actionDraft.trim();
    if (!title) return;
    setActionDraft("");
    persistManualActions([...manualActions, { title, done: false }]);
  }, [actionDraft, manualActions, persistManualActions]);

  const handleToggleManualAction = useCallback(
    (index: number) => {
      persistManualActions(manualActions.map((a, i) => (i === index ? { ...a, done: !a.done } : a)));
    },
    [manualActions, persistManualActions],
  );

  const handleRemoveManualAction = useCallback(
    (index: number) => {
      persistManualActions(manualActions.filter((_, i) => i !== index));
    },
    [manualActions, persistManualActions],
  );

  const handleAddTag = useCallback(async () => {
    const tag = tagDraft.trim().replace(/^#+/, "").trim();
    if (!tag || !selectedMeeting) {
      setAddingTag(false);
      setTagDraft("");
      return;
    }
    try {
      const updated = await invoke<string[]>("meeting_engine_add_user_tag", {
        meetingId: selectedMeeting.id,
        tag,
      });
      setUserTags(updated);
    } catch (err) {
      console.warn("[meeting] add tag failed:", err);
    } finally {
      setTagDraft("");
      setAddingTag(false);
    }
  }, [selectedMeeting, tagDraft]);

  const handleRemoveTag = useCallback(
    async (tag: string) => {
      if (!selectedMeeting) return;
      try {
        const updated = await invoke<string[]>("meeting_engine_remove_user_tag", {
          meetingId: selectedMeeting.id,
          tag,
        });
        setUserTags(updated);
      } catch (err) {
        console.warn("[meeting] remove tag failed:", err);
      }
    },
    [selectedMeeting],
  );

  const handleDismissAiTag = useCallback(
    async (tag: string) => {
      if (!selectedMeeting) return;
      try {
        await invoke("meeting_engine_dismiss_ai_tag", { meetingId: selectedMeeting.id, tag });
        await refreshOverviews();
      } catch (err) {
        console.warn("[meeting] dismiss tag failed:", err);
      }
    },
    [selectedMeeting, refreshOverviews],
  );

  const displayTitle = selectedMeeting
    ? overviews[selectedMeeting.id]?.title?.trim()
      || meetingAi?.title?.trim()
      || selectedMeeting.title
    : "";
  const isFavorite = selectedMeeting ? (overviews[selectedMeeting.id]?.favorite ?? false) : false;

  // Context passed to the AI chat: the user's notes plus their manual actions.
  const chatContext = [
    notes.trim(),
    manualActions.length > 0
      ? `Manual action items:\n${manualActions
          .map((action) => `- ${action.title}${action.done ? " (done)" : ""}`)
          .join("\n")}`
      : "",
  ]
    .filter(Boolean)
    .join("\n\n");

  const startEditTitle = useCallback(() => {
    setTitleDraft(displayTitle);
    setEditingTitle(true);
  }, [displayTitle]);

  const saveTitle = useCallback(async () => {
    if (!selectedMeeting) {
      setEditingTitle(false);
      return;
    }
    const next = titleDraft.trim();
    setEditingTitle(false);
    try {
      // Empty reverts to the AI/server title (override cleared).
      await invoke("meeting_engine_set_meeting_title", {
        meetingId: selectedMeeting.id,
        title: next.length > 0 ? next : null,
      });
      await refreshOverviews();
    } catch (err) {
      console.warn("[meeting] set title failed:", err);
    }
  }, [selectedMeeting, titleDraft, refreshOverviews]);

  const setMeetingFavorite = useCallback(async (meetingId: string, favorite: boolean) => {
    setOverviews((current) => {
      const overview = current[meetingId];
      if (!overview) return current;
      return { ...current, [meetingId]: { ...overview, favorite } };
    });
    try {
      await invoke("meeting_engine_set_meeting_favorite", {
        meetingId,
        favorite,
      });
      await refreshOverviews();
    } catch (err) {
      console.warn("[meeting] set favorite failed:", err);
      await refreshOverviews();
    }
  }, [refreshOverviews]);

  const toggleFavorite = useCallback(async () => {
    if (!selectedMeeting) return;
    await setMeetingFavorite(selectedMeeting.id, !isFavorite);
  }, [selectedMeeting, isFavorite, setMeetingFavorite]);

  const handleHideMeeting = useCallback(async () => {
    if (!selectedMeeting) return;
    const target = { id: selectedMeeting.id, title: displayTitle };
    try {
      await invoke("meeting_engine_set_meeting_hidden", { meetingId: target.id, hidden: true });
      setPendingDelete(target);
      await refreshOverviews();
    } catch (err) {
      console.warn("[meeting] hide failed:", err);
    }
  }, [selectedMeeting, displayTitle, refreshOverviews]);

  const handleRestoreMeeting = useCallback(async () => {
    if (!selectedMeeting) return;
    const id = selectedMeeting.id;
    try {
      await invoke("meeting_engine_set_meeting_hidden", { meetingId: id, hidden: false });
      setDateFilter("all");
      setSelectedMeetingId(id);
      await refreshOverviews();
    } catch (err) {
      console.warn("[meeting] restore failed:", err);
    }
  }, [selectedMeeting, refreshOverviews]);

  const handleUndoDelete = useCallback(async () => {
    if (!pendingDelete) return;
    const id = pendingDelete.id;
    setPendingDelete(null);
    try {
      await invoke("meeting_engine_set_meeting_hidden", { meetingId: id, hidden: false });
      setSelectedMeetingId(id);
      await refreshOverviews();
    } catch (err) {
      console.warn("[meeting] undo failed:", err);
    }
  }, [pendingDelete, refreshOverviews]);

  const handleDeleteFiles = useCallback(async () => {
    if (!pendingDelete) return;
    const id = pendingDelete.id;
    setPendingDelete(null);
    try {
      // Local-only permanent delete: remove the on-device artifacts.
      await invoke("meeting_engine_delete_meeting_files", { meetingId: id });
      dismissedProcessingIdsRef.current.delete(id);
      setProcStatus((current) => (current?.meeting_id === id ? null : current));
      setArtifacts((current) => (selectedIdRef.current === id ? null : current));
      setSelectedMeetingId((cur) => (cur === id ? null : cur));
      await fetchMeetings();
    } catch (err) {
      console.warn("[meeting] delete files failed:", err);
    }
  }, [pendingDelete, fetchMeetings]);

  const detailTabs: Array<{ id: DetailTab; label: string; icon: ReactNode }> = [
    { id: "summary", label: "Summary", icon: <Sparkles size={15} /> },
    { id: "notes", label: "My Notes", icon: <FileText size={15} /> },
    { id: "transcript", label: "Transcript", icon: <ScrollText size={15} /> },
    { id: "actions", label: `Actions & Decisions ${selectedActionDecisionCount}`, icon: <ListChecks size={15} /> },
    { id: "chat", label: "AI Chat", icon: <MessageSquare size={15} /> },
  ];

  return (
    <div
      className="flex h-full flex-col overflow-hidden"
      style={{ background: "hsl(var(--surface-2))" }}
    >
      {hasModel === false ? (
        <div
          className="flex flex-wrap items-center gap-3 px-5 py-2.5"
          style={{ background: "hsl(var(--chip-amber-bg))", borderBottom: "1px solid hsl(var(--chip-amber-fg) / 0.22)" }}
        >
          <AlertTriangle size={15} className="flex-shrink-0" style={{ color: "hsl(var(--chip-amber-fg))" }} />
          <span className="min-w-0 flex-1 text-[12px] text-foreground">
            <span className="font-semibold">Transcription model not installed yet.</span> Meetings
            can't be transcribed until the model finishes downloading.
          </span>
          <button
            type="button"
            onClick={() => void downloadMeetingModel()}
            disabled={downloadingModel}
            className="flex h-7 flex-shrink-0 items-center gap-1.5 rounded-lg px-3 text-[12px] font-bold disabled:opacity-70"
            style={{ background: "hsl(var(--chip-amber-fg))", color: "hsl(var(--background))" }}
          >
            {downloadingModel ? <Loader2 size={13} className="animate-spin" /> : null}
            {downloadingModel ? "Downloading…" : "Download model"}
          </button>
        </div>
      ) : null}
      <div
        className="flex flex-shrink-0 items-center px-4 py-2.5"
        style={{ borderBottom: "1px solid hsl(var(--border))" }}
      >
        <div className="seg" role="tablist" aria-label="Meetings sections">
          {(
            [
              { id: "meetings" as const, label: "Meetings", icon: <Video size={13} /> },
              { id: "digest" as const, label: "Digest", icon: <Layers size={13} /> },
            ]
          ).map((t) => (
            <button
              key={t.id}
              type="button"
              role="tab"
              aria-selected={viewMode === t.id}
              onClick={() => setViewMode(t.id)}
              className="h-7 px-3 text-[12px]"
            >
              {t.icon}
              {t.label}
            </button>
          ))}
        </div>
      </div>
      {/* Kept mounted (hidden via CSS) so a generated digest survives tab switches. */}
      <div className={`min-h-0 flex-1 overflow-hidden ${viewMode === "digest" ? "flex" : "hidden"}`}>
        <DigestView meetings={meetings} overviews={overviews} />
      </div>
      <div
        className={`relative min-h-0 flex-1 overflow-hidden ${viewMode === "meetings" ? "flex" : "hidden"}`}
      >
      <div className="flex-1 overflow-y-auto">
        <div className="mx-auto w-full max-w-[860px] px-6 pt-6 pb-16">
          <div className="flex items-center justify-between">
            <div>
              <h1 className="text-[24px] font-bold tracking-tight text-foreground leading-tight">Meetings</h1>
              <p className="mt-1 flex items-center gap-2 text-[12.5px] text-muted-foreground">
                <span
                  className="inline-block h-1.5 w-1.5 rounded-full"
                  style={{ background: "hsl(var(--accent-violet))", boxShadow: "0 0 8px hsl(var(--accent-violet) / 0.5)" }}
                />
                {liveCount} live · {endedCount} ended
              </p>
            </div>
            <div className="flex items-center gap-1.5">
              <IconButton
                label="Clear empty meetings"
                disabled={clearing || loading}
                onClick={() => void handleClearEmpty()}
              >
                <Eraser size={13} className={clearing ? "animate-pulse" : ""} />
              </IconButton>
              <IconButton label="Refresh meetings" disabled={loading} onClick={() => void fetchMeetings()}>
                <RefreshCw size={13} className={loading ? "animate-spin" : ""} />
              </IconButton>
              <button
                onClick={handleNewMeeting}
                disabled={creating || hasModel === false}
                className="flex h-8 w-8 items-center justify-center rounded-lg transition-colors disabled:cursor-not-allowed disabled:opacity-40"
                style={{ background: "hsl(var(--primary))", color: "hsl(var(--primary-foreground))" }}
                title={hasModel === false ? "Install a transcription model first" : "New Meeting"}
              >
                {creating ? <Loader2 size={13} className="animate-spin" /> : <Plus size={13} />}
              </button>
            </div>
          </div>
          <div className="mt-3 flex h-9 items-center gap-2 rounded-lg px-3" style={{ background: "hsl(var(--input))", border: "1px solid hsl(var(--border))" }}>
            {searchBusy ? (
              <Loader2 size={13} className="animate-spin text-muted-foreground" />
            ) : (
              <Search size={13} className="text-muted-foreground" />
            )}
            <input
              value={searchQuery}
              onChange={(event) => setSearchQuery(event.currentTarget.value)}
              onKeyDown={(event) => {
                if (event.key === "Escape") setSearchQuery("");
              }}
              placeholder="Search title, tags, summary, notes, transcript…"
              className="h-full min-w-0 flex-1 bg-transparent text-[12px] text-foreground outline-none placeholder:text-muted-foreground"
            />
            {searchQuery ? (
              <button
                type="button"
                onClick={() => setSearchQuery("")}
                className="text-muted-foreground hover:text-foreground"
                title="Clear search"
              >
                <X size={13} />
              </button>
            ) : null}
          </div>
          <div className="mt-3 flex flex-nowrap items-center gap-1 overflow-x-auto" style={{ scrollbarWidth: "none" }}>
            {[
              { id: "all" as const, label: "All" },
              { id: "favorites" as const, label: "Favorites" },
              { id: "today" as const, label: "Today" },
              { id: "week" as const, label: "Week" },
              { id: "archived" as const, label: "Archived" },
            ].map((filter) => (
              <button
                key={filter.id}
                type="button"
                onClick={() => setDateFilter(filter.id)}
                data-active={dateFilter === filter.id}
                className="h-7 shrink-0 rounded-lg px-2.5 text-[11px] font-semibold transition-colors"
                style={{
                  background: dateFilter === filter.id ? "hsl(var(--surface-4))" : "transparent",
                  color: dateFilter === filter.id ? "hsl(var(--foreground))" : "hsl(var(--muted-foreground))",
                  boxShadow: dateFilter === filter.id ? "inset 0 0 0 1px hsl(var(--border))" : "none",
                }}
              >
                {filter.label}
              </button>
            ))}
          </div>

          {recentTags.length > 0 || activeTag ? (
            <div className="mt-4">
              <p className="section-label mb-2">Recent tags</p>
              <div className="flex flex-wrap gap-1.5">
                {activeTag ? (
                  <button type="button" className="tag-chip" data-active="true" onClick={() => setActiveTag(null)}>
                    #{activeTag}
                    <X size={11} className="ml-1" />
                  </button>
                ) : null}
                {recentTags
                  .filter((tag) => tag !== activeTag)
                  .map((tag) => (
                    <button key={tag} type="button" className="tag-chip" onClick={() => setActiveTag(tag)}>
                      #{tag}
                    </button>
                  ))}
              </div>
            </div>
          ) : null}

          {pendingDelete ? (
            <div
              className="mt-4 flex w-full flex-wrap items-center gap-x-4 gap-y-2 rounded-xl px-4 py-2.5"
              style={{ background: "hsl(var(--surface-3))", border: "1px solid hsl(var(--border))" }}
            >
              <div className="flex min-w-0 flex-1 items-center gap-2.5">
                <Trash2 size={15} className="flex-shrink-0" style={{ color: "hsl(354 80% 70%)" }} />
                <p className="min-w-0 truncate text-[13px] text-muted-foreground">
                  <span className="font-semibold text-foreground">{pendingDelete.title}</span> removed — files still on disk.
                </p>
              </div>
              <div className="flex flex-shrink-0 items-center gap-1">
                <button
                  type="button"
                  onClick={() => void handleUndoDelete()}
                  className="h-8 rounded-lg px-3 text-[12px] font-semibold"
                  style={{ background: "hsl(var(--surface-4))", color: "hsl(var(--foreground))" }}
                >
                  Undo
                </button>
                <button
                  type="button"
                  onClick={() => void handleDeleteFiles()}
                  className="h-8 rounded-lg px-3 text-[12px] font-semibold transition-colors hover:bg-[hsl(354_60%_18%)]"
                  style={{ color: "hsl(354 82% 72%)" }}
                >
                  Delete files
                </button>
                <button
                  type="button"
                  onClick={() => setPendingDelete(null)}
                  title="Dismiss"
                  className="ml-0.5 flex h-8 w-8 items-center justify-center rounded-lg text-muted-foreground transition-colors hover:text-foreground"
                >
                  <X size={15} />
                </button>
              </div>
            </div>
          ) : null}

          <div className="mt-5">
          {loading && meetings.length === 0 && !hasLoadedRef.current ? (
            <div className="space-y-2">
              <Skeleton className="mx-1 mb-1 h-2.5 w-24" />
              {Array.from({ length: 7 }).map((_, i) => (
                <div
                  key={i}
                  className="flex items-center gap-3 rounded-xl p-3"
                  style={{ background: "hsl(var(--surface-3))", border: "1px solid hsl(var(--border))" }}
                >
                  <div className="min-w-0 flex-1">
                    <Skeleton className="h-3.5" style={{ width: `${68 - (i % 3) * 12}%` }} />
                    <Skeleton className="mt-2 h-2.5 w-2/5" />
                  </div>
                  <Skeleton className="h-4 w-4 rounded" />
                </div>
              ))}
            </div>
          ) : error ? (
            <div className="flex flex-col items-center justify-center gap-3 py-24">
              <p className="text-[12px] text-muted-foreground">{error}</p>
              <button
                onClick={() => void fetchMeetings()}
                className="rounded-lg px-3 py-1.5 text-[11px] font-medium transition-colors"
                style={{ background: "hsl(var(--surface-3))", color: "hsl(var(--foreground))" }}
              >
                Retry
              </button>
            </div>
          ) : sortedMeetings.length === 0 ? (
            <div className="flex flex-col items-center justify-center gap-3 py-24 opacity-60">
              <Video size={28} className="text-muted-foreground" />
              <p className="text-[12px] text-muted-foreground">
                {searching
                  ? searchBusy
                    ? "Searching…"
                    : `No meetings match “${searchQuery.trim()}”`
                  : emptyMeetingMessage}
              </p>
            </div>
          ) : (
            <div className="space-y-2">
              <p className="section-label px-1 pb-1">
                {searching ? `${sortedMeetings.length} result${sortedMeetings.length === 1 ? "" : "s"}` : sectionLabel}
              </p>
              {sortedMeetings.map((meeting) => {
                const ov = overviews[meeting.id];
                const rowTitle = ov?.title?.trim() || meeting.title;
                const rowWords = ov?.word_count ?? wordCount(meeting.agenda);
                const rowTags = ov?.tags ?? [];
                const rowFavorite = ov?.favorite ?? false;
                const hit = searching ? searchHits?.[meeting.id] : undefined;
                const openMeeting = () => {
                  setSelectedMeetingId(meeting.id);
                  setDetailTab("summary");
                };
                return (
                  <div
                    key={meeting.id}
                    role="button"
                    tabIndex={0}
                    className="meeting-row"
                    onClick={openMeeting}
                    onKeyDown={(event) => {
                      if (event.key === "Enter" || event.key === " ") {
                        event.preventDefault();
                        openMeeting();
                      }
                    }}
                  >
                    <div className="min-w-0 flex-1">
                      <div className="flex items-center gap-2">
                        <h3 className="truncate text-[13.5px] font-semibold text-foreground">{rowTitle}</h3>
                        {meeting.status === "live" ? (
                          <span
                            className="flex-shrink-0 rounded-md px-1.5 py-0.5 text-[9px] font-bold uppercase tracking-wide"
                            style={{ background: "hsl(var(--recording) / 0.16)", color: "hsl(var(--recording))" }}
                          >
                            Live
                          </span>
                        ) : null}
                      </div>
                      <p className="mt-1 flex flex-wrap items-center gap-x-2 gap-y-0.5 text-[11px] text-muted-foreground">
                        <span className="tabular-nums">{formatMeetingDate(meeting)}</span>
                        <span className="opacity-40">·</span>
                        <span className="tabular-nums">{rowWords} words</span>
                        {(ov?.action_count ?? 0) > 0 ? (
                          <>
                            <span className="opacity-40">·</span>
                            <span className="tabular-nums">{ov?.action_count} actions</span>
                          </>
                        ) : null}
                        {rowTags.length > 0 ? (
                          <>
                            <span className="opacity-40">·</span>
                            <span className="truncate" style={{ color: tagColor(rowTags[0]) }}>
                              #{rowTags[0]}{rowTags.length > 1 ? ` +${rowTags.length - 1}` : ""}
                            </span>
                          </>
                        ) : null}
                      </p>
                      {hit?.snippet ? (
                        <p className="mt-1 line-clamp-1 text-[11px] text-muted-foreground/80">{hit.snippet}</p>
                      ) : null}
                    </div>
                    <button
                      type="button"
                      title={rowFavorite ? "Remove from favorites" : "Add to favorites"}
                      aria-label={rowFavorite ? "Remove from favorites" : "Add to favorites"}
                      onClick={(event) => {
                        event.stopPropagation();
                        void setMeetingFavorite(meeting.id, !rowFavorite);
                      }}
                      onKeyDown={(event) => event.stopPropagation()}
                      className="flex-shrink-0 rounded-md p-1 transition-colors hover:text-foreground"
                      style={{ color: rowFavorite ? "hsl(38 90% 72%)" : "hsl(var(--muted-foreground) / 0.4)" }}
                    >
                      <Star size={15} fill={rowFavorite ? "hsl(38 90% 72%)" : "none"} />
                    </button>
                  </div>
                );
              })}
            </div>
          )}
        </div>
        </div>
      </div>
      </div>

      {selectedMeeting ? (
        <div className="meeting-modal-overlay" onClick={() => setSelectedMeetingId(null)}>
          <div
            className="meeting-modal-card"
            role="dialog"
            aria-modal="true"
            onClick={(event) => event.stopPropagation()}
          >
            <button
              type="button"
              className="meeting-modal-close"
              title="Close"
              onClick={() => setSelectedMeetingId(null)}
            >
              <X size={16} />
            </button>
            <div className="meeting-modal-body">
              <div className="mx-auto w-full max-w-[1080px] px-6 pb-12 pt-8">
            <div className="flex items-start justify-between gap-5">
              <div className="min-w-0">
                {editingTitle ? (
                  <input
                    autoFocus
                    value={titleDraft}
                    onChange={(event) => setTitleDraft(event.currentTarget.value)}
                    onKeyDown={(event) => {
                      if (event.key === "Enter") {
                        event.preventDefault();
                        void saveTitle();
                      } else if (event.key === "Escape") {
                        setEditingTitle(false);
                      }
                    }}
                    onBlur={() => void saveTitle()}
                    placeholder="Meeting title"
                    maxLength={120}
                    className="w-full bg-transparent text-[22px] font-bold tracking-tight text-foreground outline-none"
                    style={{ borderBottom: "2px solid hsl(var(--accent-violet) / 0.6)" }}
                  />
                ) : (
                  <h2 className="truncate text-[22px] font-bold tracking-tight text-foreground">{displayTitle}</h2>
                )}
                <div className="mt-2.5 flex flex-wrap items-center gap-x-2 gap-y-1 text-[12px] text-muted-foreground">
                  <span className="tabular-nums">{formatMeetingDate(selectedMeeting)}</span>
                  <span className="opacity-40">·</span>
                  <span className="tabular-nums">{formatTimestamp((artifacts?.audio_duration_ms ?? audioDuration * 1000) || 0)}</span>
                  <span className="opacity-40">·</span>
                  <span>{selectedMeeting.status === "live" ? "Live" : "Local"}</span>
                  <span className="opacity-40">·</span>
                  <span className="tabular-nums">{selectedMeeting.participants_count ?? 0}p</span>
                  <span className="opacity-40">·</span>
                  <span className="tabular-nums">{selectedWordCount} words</span>
                  {meetingAi?.model ? (
                    <span
                      className="ml-1 rounded-md px-2 py-0.5 text-[10.5px] font-medium"
                      style={{ background: "hsl(var(--surface-4))", color: "hsl(var(--muted-foreground))" }}
                    >
                      {meetingAi.transcript_source} · {meetingAi.model}
                    </span>
                  ) : null}
                </div>
                <div className="mt-4 flex flex-wrap items-center gap-1.5">
                  {addingTag ? (
                    <input
                      autoFocus
                      value={tagDraft}
                      onChange={(event) => setTagDraft(event.currentTarget.value)}
                      onKeyDown={(event) => {
                        if (event.key === "Enter") {
                          event.preventDefault();
                          void handleAddTag();
                        } else if (event.key === "Escape") {
                          setAddingTag(false);
                          setTagDraft("");
                        }
                      }}
                      onBlur={() => void handleAddTag()}
                      placeholder="Tag name"
                      maxLength={32}
                      className="h-7 w-32 rounded-full bg-transparent px-3 text-[11.5px] font-semibold outline-none"
                      style={{ border: "1px solid hsl(var(--accent-violet) / 0.5)", color: "hsl(var(--foreground))" }}
                    />
                  ) : (
                    <button
                      type="button"
                      onClick={() => {
                        setTagDraft("");
                        setAddingTag(true);
                      }}
                      className="flex h-7 items-center gap-1 rounded-full border border-dashed px-3 text-[11.5px] font-semibold text-muted-foreground transition-colors hover:text-foreground"
                      style={{ borderColor: "hsl(var(--border))" }}
                    >
                      <Plus size={12} /> Add tag
                    </button>
                  )}
                  {tags.map((tag) => (
                    <span
                      key={`${tag.source}-${tag.label}`}
                      className="group inline-flex h-7 items-center gap-1 rounded-full px-2.5 text-[11.5px] font-semibold"
                      style={{ background: `${tag.color}1f`, color: tag.color }}
                    >
                      #{tag.label}
                      <button
                        type="button"
                        onClick={() =>
                          tag.source === "user"
                            ? void handleRemoveTag(tag.label)
                            : void handleDismissAiTag(tag.label)
                        }
                        title="Remove tag"
                        className="ml-0.5 flex w-0 items-center overflow-hidden opacity-0 transition-all group-hover:w-3.5 group-hover:opacity-80 hover:!opacity-100"
                      >
                        <X size={12} />
                      </button>
                    </span>
                  ))}
                </div>
              </div>
              <div className="flex items-center gap-3 text-muted-foreground">
                <button type="button" title="Edit title" onClick={startEditTitle} className="transition-colors hover:text-foreground">
                  <Pencil size={17} />
                </button>
                <button
                  type="button"
                  title={isFavorite ? "Remove from favorites" : "Add to favorites"}
                  onClick={() => void toggleFavorite()}
                  className="transition-colors hover:text-foreground"
                  style={{ color: isFavorite ? "hsl(38 90% 72%)" : undefined }}
                >
                  <Star size={18} fill={isFavorite ? "hsl(38 90% 72%)" : "none"} />
                </button>
                <button type="button" title="AI Chat" onClick={() => setDetailTab("chat")} className="transition-colors hover:text-foreground">
                  <MessageSquare size={18} />
                </button>
                {overviews[selectedMeeting.id]?.hidden ? (
                  <button
                    type="button"
                    title="Restore to list"
                    onClick={() => void handleRestoreMeeting()}
                    className="flex items-center gap-1.5 text-[12px] font-semibold transition-colors hover:text-foreground"
                  >
                    <RotateCcw size={16} />
                    Restore
                  </button>
                ) : (
                  <button
                    type="button"
                    title="Remove meeting"
                    onClick={() => void handleHideMeeting()}
                    className="transition-colors hover:text-[hsl(354_85%_70%)]"
                  >
                    <Trash2 size={17} />
                  </button>
                )}
              </div>
            </div>

            <MeetingAudioBar
              audioSrc={audioSrc}
              audioRef={audioRef}
              currentTime={audioCurrentTime}
              duration={audioDuration}
              playing={audioPlaying}
              speed={audioRate}
              fallbackDurationMs={artifacts?.audio_duration_ms}
              onDuration={setAudioDuration}
              onTime={setAudioCurrentTime}
              onToggle={toggleAudio}
              onSeek={seekAudio}
              onSpeed={cycleSpeed}
              onDownload={handleDownloadAudio}
              downloading={downloadingAudio}
            />

            {procStatus && (procStatus.running || procStatus.can_retry || procStatus.summary_failed || procStatus.error) ? (
              <ProcessingBanner
                status={procStatus}
                onRetryTranscribe={handleRetranscribe}
                onRetrySummary={handleReanalyze}
                onCancel={handleCancelProcessing}
                onDismiss={handleDismissProcessingBanner}
                retrying={retranscribing || meetingAiLoading}
              />
            ) : null}

            <div className="mt-6 grid grid-cols-5 border-b" style={{ borderColor: "hsl(var(--border))" }}>
              {detailTabs.map((tab) => (
                <button
                  key={tab.id}
                  type="button"
                  onClick={() => setDetailTab(tab.id)}
                  title={tab.label}
                  className="flex h-11 min-w-0 items-center justify-center gap-1.5 px-1 text-[12.5px] font-semibold transition-colors lg:gap-2"
                  style={{
                    color: detailTab === tab.id ? "hsl(var(--foreground))" : "hsl(var(--muted-foreground))",
                    borderBottom: detailTab === tab.id ? "2px solid hsl(var(--accent-violet))" : "2px solid transparent",
                  }}
                >
                  <span className="flex-shrink-0">{tab.icon}</span>
                  <span className="truncate">{tab.label}</span>
                </button>
              ))}
            </div>

            {detailTab === "summary" ? (
              <div className="pt-6">
                <div className="mb-5 flex items-center justify-between gap-3">
                  <h3 className="text-[14px] font-semibold text-foreground">Summary</h3>
                  <div className="flex items-center gap-2">
                    <ToolbarButton
                      icon={<RefreshCw size={14} className={meetingAiLoading ? "animate-spin" : ""} />}
                      label={
                        procStatus?.running
                          ? "Processing…"
                          : meetingAiLoading
                            ? "Analyzing…"
                            : meetingAi?.summary
                              ? "Reanalyse"
                              : "Generate"
                      }
                      disabled={meetingAiLoading || Boolean(procStatus?.running)}
                      onClick={handleReanalyze}
                    />
                    <CopyButton label="Copy" disabled={!meetingAi?.summary} onCopy={copySummary} />
                    {(() => {
                      const larkUrl = selectedMeeting
                        ? overviews[selectedMeeting.id]?.lark_doc_url ?? null
                        : null;
                      return (
                        <>
                          <ToolbarButton
                            icon={<ExternalLink size={14} />}
                            label={
                              larkExporting
                                ? "Exporting…"
                                : larkUrl
                                  ? "Re-export to Lark"
                                  : "Export to Lark"
                            }
                            disabled={!meetingAi?.summary || larkExporting}
                            onClick={() => void handleExportToLark()}
                          />
                          {larkUrl ? (
                            <button
                              type="button"
                              onClick={() => void openExternal(larkUrl)}
                              className="h-9 rounded-lg px-3 text-[12px] font-semibold transition-colors"
                              style={{
                                background: "hsl(var(--chip-lime-bg))",
                                border: "1px solid hsl(var(--chip-lime-fg) / 0.26)",
                                color: "hsl(var(--chip-lime-fg))",
                              }}
                            >
                              <span className="flex items-center gap-2">
                                <ExternalLink size={14} /> Open in Lark
                              </span>
                            </button>
                          ) : null}
                        </>
                      );
                    })()}
                  </div>
                </div>
                {larkError ? (
                  <div className="mb-3 flex flex-wrap items-center gap-2">
                    <p className="text-[12px]" style={{ color: "hsl(0 70% 70%)" }}>
                      Lark export: {larkError}
                    </p>
                    {larkNeedsReauth ? (
                      <button
                        type="button"
                        onClick={() => onOpenWorkspaces?.()}
                        className="rounded-md px-2.5 py-1 text-[11px] font-semibold transition-colors"
                        style={{
                          background: "hsl(var(--primary))",
                          color: "hsl(var(--primary-foreground))",
                        }}
                      >
                        Reconnect Lark
                      </button>
                    ) : null}
                  </div>
                ) : null}
                {meetingAiLoading ? (
                  <div className="flex items-center gap-3 text-[14px] text-muted-foreground">
                    <Loader2 size={16} className="animate-spin" />
                    Loading generated summary
                  </div>
                ) : meetingAiError ? (
                  <p className="text-[14px]" style={{ color: "hsl(354 85% 75%)" }}>{meetingAiError}</p>
                ) : meetingAi?.summary?.trim() ? (
                  <>
                    <div className="rounded-xl p-5" style={{ background: "hsl(var(--surface-3))", border: "1px solid hsl(var(--border))" }}>
                      <div className="mb-2.5 flex items-center gap-2">
                        <Sparkles size={14} style={{ color: "hsl(var(--accent-violet))" }} />
                        <span className="section-label">AI summary</span>
                      </div>
                      <p className="max-w-[92ch] text-[14.5px] leading-relaxed text-foreground">
                        {stripInlineMarkdown(summaryLead(meetingAi.summary))}
                      </p>
                    </div>
                    <DecisionsBlock decisions={meetingAi.decisions ?? []} />
                    <MeetingSummaryContent summary={meetingAi.summary} />
                  </>
                ) : (
                  <p className="text-[14px] text-muted-foreground">
                    {artifactsLoading ? "Loading meeting artifacts..." : "Summary is not generated yet."}
                  </p>
                )}
              </div>
            ) : null}

            {detailTab === "notes" ? (
              <div className="pt-6">
                <div className="mb-5 flex items-center justify-between gap-3">
                  <div className="flex items-center gap-3">
                    <h3 className="text-[14px] font-semibold text-foreground">My Notes</h3>
                    <span className="text-[11px] font-semibold text-muted-foreground">
                      {notesStatus === "saving" ? "Saving…" : notesStatus === "saved" ? "Saved ✓" : ""}
                    </span>
                  </div>
                  {notes.trim() ? (
                    <div className="flex items-center gap-1 rounded-lg p-0.5" style={{ background: "hsl(var(--surface-3))", border: "1px solid hsl(var(--border))" }}>
                      <button
                        type="button"
                        onClick={() => setNotesPreview(true)}
                        className="rounded-md px-3 py-1 text-[12px] font-bold transition-colors"
                        style={notesPreview ? { background: "hsl(var(--primary) / 0.16)", color: "hsl(var(--primary))" } : { color: "hsl(var(--muted-foreground))" }}
                      >
                        Preview
                      </button>
                      <button
                        type="button"
                        onClick={() => setNotesPreview(false)}
                        className="rounded-md px-3 py-1 text-[12px] font-bold transition-colors"
                        style={!notesPreview ? { background: "hsl(var(--primary) / 0.16)", color: "hsl(var(--primary))" } : { color: "hsl(var(--muted-foreground))" }}
                      >
                        Write
                      </button>
                    </div>
                  ) : null}
                </div>
                {notesPreview && notes.trim() ? (
                  <div className="rounded-xl px-5 py-4" style={{ background: "hsl(var(--surface-3))", border: "1px solid hsl(var(--border))" }}>
                    <MeetingRichText text={notes} />
                  </div>
                ) : (
                  <div className="rounded-xl" style={{ background: "hsl(var(--surface-3))", border: "1px solid hsl(var(--border))" }}>
                    <textarea
                      value={notes}
                      onChange={(event) => handleNotesChange(event.currentTarget.value)}
                      placeholder="Jot down anything from this meeting — decisions, follow-ups, your own takeaways. Markdown supported. These notes are private to you and are used as extra context when you ask the AI Chat about this meeting."
                      className="min-h-[340px] w-full resize-y rounded-xl bg-transparent px-5 py-4 text-[14px] leading-relaxed text-foreground outline-none placeholder:text-muted-foreground"
                      spellCheck
                    />
                  </div>
                )}
                <p className="mt-3 flex items-center gap-2 text-[12px] text-muted-foreground">
                  <Sparkles size={13} style={{ color: "hsl(var(--accent-violet))" }} />
                  Your notes are included as context in this meeting's AI Chat.
                </p>
              </div>
            ) : null}

            {detailTab === "transcript" ? (
              <TranscriptTab
                artifacts={artifacts}
                onSeekToSegment={seekToSegment}
                onCopyTranscript={copyTranscript}
                onRetranscribe={handleRetranscribe}
                retranscribing={retranscribing || Boolean(procStatus?.running)}
              />
            ) : null}

            {detailTab === "actions" ? (
              <ActionRows
                meetingAi={meetingAi}
                completed={completedActions}
                onToggle={(key) => {
                  setCompletedActions((current) => {
                    const next = new Set(current);
                    if (next.has(key)) next.delete(key);
                    else next.add(key);
                    return next;
                  });
                }}
                manualActions={manualActions}
                actionDraft={actionDraft}
                onActionDraftChange={setActionDraft}
                onAddManualAction={handleAddManualAction}
                onToggleManualAction={handleToggleManualAction}
                onRemoveManualAction={handleRemoveManualAction}
              />
            ) : null}

            {detailTab === "chat" ? (
              <div className="pt-6">
                <h3 className="mb-5 text-[14px] font-semibold text-foreground">AI Chat</h3>
                <div
                  className="h-[560px] overflow-hidden rounded-xl"
                  style={{ background: "hsl(var(--surface-3))", border: "1px solid hsl(var(--border))" }}
                >
                  <MeetingAiChat
                    resetKey={selectedMeeting.id}
                    summary={meetingAi?.summary ?? null}
                    transcriptOverride={artifacts?.transcript ?? null}
                    notes={chatContext}
                    canSend={Boolean(artifacts?.transcript) || Boolean(meetingAi?.summary)}
                    unavailableLabel="This meeting has no transcript or summary to chat about yet."
                    emptyHint="Ask about this recording. Answers use the selected meeting transcript and generated MoM."
                    placeholder="Ask about this recording…"
                    onSeek={seekToSeconds}
                  />
                </div>
              </div>
            ) : null}
              </div>
            </div>
          </div>
        </div>
      ) : null}

      {meetingInProgress && (
        <div
          className="fixed inset-0 z-[100] flex items-center justify-center p-6"
          style={{ background: "hsl(0 0% 0% / 0.55)" }}
          onClick={() => setMeetingInProgress(false)}
        >
          <div
            className="w-full max-w-sm rounded-2xl p-5"
            style={{ background: "hsl(var(--surface-3))", border: "1px solid hsl(var(--border))" }}
            onClick={(e) => e.stopPropagation()}
          >
            <h3 className="text-[15px] font-bold text-foreground">A meeting is already in progress</h3>
            <p className="mt-1.5 text-[13px] text-muted-foreground">
              Finish the current meeting before starting a new one.
            </p>
            <div className="mt-4 flex justify-end gap-2">
              <button
                type="button"
                onClick={() => setMeetingInProgress(false)}
                className="h-9 rounded-lg px-3 text-[12px] font-bold"
                style={{ background: "hsl(var(--surface-4))", color: "hsl(var(--foreground))" }}
              >
                Cancel
              </button>
              {activeMeetingId ? (
                <button
                  type="button"
                  onClick={() => {
                    setMeetingInProgress(false);
                    onJoinMeeting?.(activeMeetingId);
                  }}
                  className="h-9 rounded-lg px-3 text-[12px] font-bold"
                  style={{ background: "hsl(var(--primary))", color: "hsl(var(--primary-foreground))" }}
                >
                  Open it
                </button>
              ) : null}
            </div>
          </div>
        </div>
      )}

      {processingStartWarning && (
        <div
          className="fixed inset-0 z-[100] flex items-center justify-center p-6"
          style={{ background: "hsl(0 0% 0% / 0.55)" }}
          onClick={() => {
            if (!creating) setProcessingStartWarning(null);
          }}
        >
          <div
            className="w-full max-w-md rounded-2xl p-5"
            style={{ background: "hsl(var(--surface-3))", border: "1px solid hsl(var(--border))" }}
            onClick={(e) => e.stopPropagation()}
          >
            <div className="flex items-start gap-3">
              <AlertTriangle
                size={18}
                className="mt-0.5 flex-shrink-0"
                style={{ color: "hsl(38 90% 72%)" }}
              />
              <div className="min-w-0">
                <h3 className="text-[15px] font-bold text-foreground">Another meeting is processing</h3>
                <p className="mt-1.5 text-[13px] leading-6 text-muted-foreground">
                  You can record now. AirNote keeps one Whisper model loaded at a time, so RAM should not double-spike,
                  but live transcript may arrive late while{" "}
                  <span className="font-semibold text-foreground">{processingStartWarning.title}</span>{" "}
                  is {processingStartWarning.queued ? "queued" : processingStageLabel(processingStartWarning.stage).toLowerCase()}.
                </p>
                <p className="mt-2 text-[13px] leading-6 text-muted-foreground">
                  For smoother live transcript, pause current processing first. You can resume it later with Re-transcribe.
                </p>
              </div>
            </div>
            <div className="mt-5 flex flex-wrap justify-end gap-2">
              <button
                type="button"
                onClick={() => setProcessingStartWarning(null)}
                disabled={creating}
                className="h-9 rounded-lg px-3 text-[12px] font-bold disabled:opacity-50"
                style={{ background: "hsl(var(--surface-4))", color: "hsl(var(--foreground))" }}
              >
                Wait
              </button>
              <button
                type="button"
                onClick={() => {
                  setProcessingStartWarning(null);
                  void startNewLocalMeeting();
                }}
                disabled={creating}
                className="h-9 rounded-lg px-3 text-[12px] font-bold disabled:opacity-50"
                style={{ background: "hsl(var(--surface-4))", color: "hsl(var(--foreground))" }}
              >
                Start anyway
              </button>
              <button
                type="button"
                onClick={() => void handlePauseProcessingAndStart()}
                disabled={creating}
                className="h-9 rounded-lg px-3 text-[12px] font-bold disabled:opacity-60"
                style={{ background: "hsl(var(--primary))", color: "hsl(var(--primary-foreground))" }}
              >
                {creating ? "Pausing…" : "Pause processing & start"}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
