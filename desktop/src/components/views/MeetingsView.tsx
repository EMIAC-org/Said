import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { formatTimestamp, speakerColor } from "@/lib/meetingFormat";
import type { MutableRefObject, ReactNode } from "react";
import {
  AlertTriangle,
  Check,
  ChevronDown,
  Copy,
  Download,
  ExternalLink,
  FileText,
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
  Users,
  Video,
  X,
} from "lucide-react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import {
  createMeeting,
  exportMeetingToLark,
  getConnection,
  listMeetings,
  repairEnterpriseConnection,
  startMeeting,
} from "@/lib/enterprise";
import { openExternal } from "@/lib/invoke";
import { MeetingAiChat } from "@/components/MeetingAiChat";
import {
  MeetingRichText,
  parseMeetingSummary,
  renderInlineMarkdown,
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

type SyncState =
  | { kind: "idle" }
  | { kind: "syncing" }
  | { kind: "done"; url: string; inSharedFolder: boolean; warning?: string | null }
  | { kind: "error"; code?: string; message: string };


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

function MeetingSummaryContent({ summary }: { summary: string }) {
  const blocks = parseMeetingSummary(summary);
  let headingSeen = false;
  return (
    <div className="mt-7 space-y-4">
      {blocks.map((block, index) => {
        if (block.kind === "heading") {
          const firstHeading = !headingSeen;
          headingSeen = true;
          return (
            <div
              key={`${block.kind}-${index}-${block.text}`}
              className={firstHeading ? "flex items-center gap-3" : "flex items-center gap-3 border-t pt-7 mt-2"}
              style={firstHeading ? undefined : { borderColor: "hsl(var(--surface-4))" }}
            >
              {block.index ? (
                <span
                  className="flex h-7 w-7 flex-shrink-0 items-center justify-center rounded-lg text-[12px] font-bold"
                  style={{ background: "hsl(var(--primary) / 0.16)", color: "hsl(var(--primary))" }}
                >
                  {block.index}
                </span>
              ) : (
                <span className="h-4 w-1 flex-shrink-0 rounded-full" style={{ background: "hsl(var(--primary))" }} />
              )}
              <h3 className="text-[18px] font-bold tracking-tight text-foreground">{block.text}</h3>
            </div>
          );
        }
        if (block.kind === "quote") {
          return (
            <blockquote
              key={`${block.kind}-${index}-${block.text}`}
              className="rounded-r-lg border-l-[3px] py-1 pl-4 text-[15px] italic leading-8 text-muted-foreground"
              style={{ borderColor: "hsl(var(--primary) / 0.7)", background: "hsl(var(--primary) / 0.05)" }}
            >
              {renderInlineMarkdown(block.text)}
            </blockquote>
          );
        }
        if (block.kind === "bullet") {
          return (
            <div key={`${block.kind}-${index}-${block.text}`} className="flex gap-3 text-[16px] leading-8 text-muted-foreground">
              {block.emoji ? (
                <span className="mt-0.5 flex-shrink-0 text-[16px] leading-8">{block.emoji}</span>
              ) : (
                <span className="mt-3.5 h-1.5 w-1.5 flex-shrink-0 rounded-full" style={{ background: "hsl(var(--primary))" }} />
              )}
              <p className="max-w-[100ch]">{renderInlineMarkdown(block.text)}</p>
            </div>
          );
        }
        return (
          <p key={`${block.kind}-${index}-${block.text}`} className="max-w-[102ch] text-[16px] leading-8 text-muted-foreground">
            {renderInlineMarkdown(block.text)}
          </p>
        );
      })}
    </div>
  );
}

function MeetingCard({
  meeting,
  overview,
  searchHit,
  selected,
  onSelect,
}: {
  meeting: Meeting;
  overview?: MeetingOverview;
  searchHit?: MeetingSearchHit;
  selected: boolean;
  onSelect: () => void;
}) {
  // Prefer the AI-generated title/word count from the cached overview, falling
  // back to the server title and agenda when a meeting hasn't been analysed.
  const title = overview?.title?.trim() || meeting.title;
  const words = overview?.word_count ?? wordCount(meeting.agenda);
  const actionCount = overview?.action_count ?? 0;
  const decisionCount = overview?.decision_count ?? 0;
  const cardTags = (overview?.tags ?? []).slice(0, 3);
  return (
    <button
      type="button"
      className="w-full rounded-xl p-4 text-left transition-colors cursor-pointer hover:brightness-110"
      style={{
        background: selected ? "hsl(var(--primary) / 0.12)" : "hsl(var(--surface-2))",
        border: selected ? "1px solid hsl(var(--primary) / 0.34)" : "1px solid hsl(var(--surface-4))",
        boxShadow: selected ? "0 0 0 1px hsl(var(--primary) / 0.08) inset" : "none",
      }}
      onClick={onSelect}
    >
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0 flex-1">
          <h3 className="truncate text-[13px] font-bold text-foreground">{title}</h3>
          <p className="mt-2 text-[11px] text-muted-foreground">
            {formatMeetingDate(meeting)} · {words} words
          </p>
        </div>
        <Star
          size={14}
          fill={overview?.favorite ? "hsl(38 90% 72%)" : "none"}
          style={{ color: overview?.favorite ? "hsl(38 90% 72%)" : "hsl(var(--surface-4))" }}
        />
      </div>
      <div className="mt-4 flex flex-wrap gap-1.5">
        <span className="rounded-full px-2 py-1 text-[10px] font-semibold" style={{ background: "hsl(0 0% 0% / 0.55)", color: "hsl(var(--primary))" }}>
          Summary
        </span>
        <span className="rounded-full px-2 py-1 text-[10px] font-semibold" style={{ background: "hsl(0 0% 0% / 0.55)", color: "hsl(142 70% 65%)" }}>
          {actionCount} actions
        </span>
        <span className="rounded-full px-2 py-1 text-[10px] font-semibold" style={{ background: "hsl(0 0% 0% / 0.55)", color: "hsl(38 90% 72%)" }}>
          {decisionCount} decisions
        </span>
      </div>
      {cardTags.length > 0 ? (
        <div className="mt-2 flex flex-wrap gap-1.5">
          {cardTags.map((tag) => (
            <span
              key={tag}
              className="rounded-md px-2 py-0.5 text-[10px] font-bold"
              style={{ background: `${tagColor(tag)}22`, color: tagColor(tag) }}
            >
              #{tag}
            </span>
          ))}
        </div>
      ) : null}
      {searchHit ? (
        <div className="mt-2.5 border-t pt-2.5" style={{ borderColor: "hsl(var(--surface-4))" }}>
          {searchHit.matched_in.length > 0 ? (
            <p className="text-[10px] font-bold uppercase tracking-[0.1em]" style={{ color: "hsl(var(--primary))" }}>
              Match · {searchHit.matched_in.join(", ")}
            </p>
          ) : null}
          {searchHit.snippet ? (
            <p className="mt-1 line-clamp-2 text-[11px] leading-5 text-muted-foreground">{searchHit.snippet}</p>
          ) : null}
        </div>
      ) : null}
    </button>
  );
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
      style={{ background: "hsl(var(--surface-3))", color: "hsl(var(--muted-foreground))" }}
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
      className="h-10 rounded-lg px-3 text-[12px] font-bold transition-colors disabled:opacity-45"
      style={{
        background: active ? "hsl(132 38% 12%)" : "hsl(var(--surface-2))",
        border: active ? "1px solid hsl(132 56% 36%)" : "1px solid hsl(var(--surface-4))",
        color: active ? "hsl(132 72% 62%)" : "hsl(var(--muted-foreground))",
      }}
    >
      <span className="flex items-center gap-2">
        {icon}
        {label}
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
  return (
    <div
      className="mt-7 flex h-14 items-center gap-4 rounded-xl px-4"
      style={{ background: "hsl(var(--surface-2))", border: "1px solid hsl(var(--surface-4))" }}
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
        className="flex h-9 w-9 flex-shrink-0 items-center justify-center rounded-full disabled:opacity-45"
        style={{ background: "hsl(var(--primary))", color: "hsl(var(--primary-foreground))" }}
        title={playing ? "Pause audio" : "Play audio"}
      >
        {playing ? <Pause size={16} /> : <Play size={17} fill="currentColor" />}
      </button>
      <input
        type="range"
        min={0}
        max={Math.max(1, effectiveDuration)}
        step={0.1}
        value={Math.min(currentTime, Math.max(1, effectiveDuration))}
        disabled={!audioSrc}
        onChange={(event) => onSeek(Number(event.currentTarget.value))}
        className="min-w-0 flex-1 accent-[hsl(var(--primary))]"
        aria-label="Audio timeline"
      />
      <div className="flex items-center gap-3 text-[12px] font-semibold text-muted-foreground">
        <span className="tabular-nums">
          {formatTimestamp(currentTime * 1000)} / {formatTimestamp(effectiveDuration * 1000)}
        </span>
        <button
          type="button"
          disabled={!audioSrc}
          onClick={() => onSeek(Math.max(0, currentTime - 10))}
          title="Back 10 seconds"
          aria-label="Back 10 seconds"
          className="relative flex h-8 w-8 items-center justify-center rounded-lg text-muted-foreground transition-colors hover:text-foreground disabled:opacity-45"
        >
          <RotateCcw size={18} />
          <span className="absolute text-[7px] font-bold">10</span>
        </button>
        <button
          type="button"
          disabled={!audioSrc}
          onClick={() => onSeek(Math.min(effectiveDuration, currentTime + 10))}
          title="Forward 10 seconds"
          aria-label="Forward 10 seconds"
          className="relative flex h-8 w-8 items-center justify-center rounded-lg text-muted-foreground transition-colors hover:text-foreground disabled:opacity-45"
        >
          <RotateCw size={18} />
          <span className="absolute text-[7px] font-bold">10</span>
        </button>
        <button
          type="button"
          disabled={!audioSrc}
          onClick={onSpeed}
          className="h-8 w-10 rounded-lg text-[12px] font-bold tabular-nums transition-colors hover:text-foreground disabled:opacity-45"
          style={{ background: "hsl(var(--surface-3))", color: "hsl(var(--muted-foreground))" }}
          title="Playback speed"
        >
          {speed}x
        </button>
        <button
          type="button"
          disabled={!audioSrc || downloading}
          onClick={onDownload}
          className="flex h-8 w-8 items-center justify-center rounded-lg text-muted-foreground transition-colors hover:text-foreground disabled:opacity-45"
          style={{ background: "hsl(var(--surface-3))" }}
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
    <div className="pt-8">
      <div className="flex items-center justify-between gap-3">
        <div className="flex items-center gap-3">
          <h3 className="text-[15px] font-bold text-foreground">Transcript</h3>
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
          <ToolbarButton icon={<Users size={14} />} label="Manage Speakers" disabled />
          <ToolbarButton icon={<Copy size={14} />} label="Copy All" disabled={!artifacts?.transcript} onClick={onCopyTranscript} />
        </div>
      </div>

      <SpeakerTimeline segments={segments} durationMs={durationMs} />

      {segments.length > 0 ? (
        <div className="mt-7 divide-y" style={{ borderColor: "hsl(var(--surface-4))" }}>
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
                <span className="mt-1 block max-w-[112ch] text-[15px] leading-7 text-muted-foreground">
                  {segment.text}
                </span>
              </span>
            </button>
          ))}
        </div>
      ) : artifacts?.transcript ? (
        <pre className="mt-7 whitespace-pre-wrap text-[15px] leading-8 text-muted-foreground">
          {artifacts.transcript}
        </pre>
      ) : (
        <p className="mt-7 text-[14px] text-muted-foreground">Transcript is not attached to this meeting yet.</p>
      )}
    </div>
  );
}

function ActionRows({
  meetingAi,
  completed,
  onToggle,
  onSync,
  syncState,
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
  onSync: () => void;
  syncState: SyncState;
  manualActions: ManualAction[];
  actionDraft: string;
  onActionDraftChange: (value: string) => void;
  onAddManualAction: () => void;
  onToggleManualAction: (index: number) => void;
  onRemoveManualAction: (index: number) => void;
}) {
  const actions = meetingAi?.action_items ?? [];
  const totalCount = actions.length + manualActions.length;
  return (
    <div className="pt-8">
      <div className="mb-7 flex items-center justify-between gap-3">
        <h3 className="text-[15px] font-bold text-foreground">Actions {totalCount}</h3>
        <button
          type="button"
          onClick={onSync}
          disabled={syncState.kind === "syncing"}
          className="h-9 rounded-lg px-3 text-[12px] font-bold disabled:opacity-45"
          style={{ background: "hsl(132 38% 12%)", color: "hsl(132 72% 62%)", border: "1px solid hsl(132 56% 32%)" }}
        >
          <span className="flex items-center gap-2">
            {syncState.kind === "syncing" ? <Loader2 size={14} className="animate-spin" /> : <ExternalLink size={14} />}
            Export to Lark
          </span>
        </button>
      </div>
      {syncState.kind === "done" ? (
        <p className="mb-5 text-[12px] font-semibold" style={{ color: "hsl(132 72% 62%)" }}>
          Exported to Lark{syncState.inSharedFolder ? " (shared folder)" : ""}
          {syncState.warning ? " — content partial" : ""}.
        </p>
      ) : syncState.kind === "error" ? (
        <p className="mb-5 text-[12px] font-semibold" style={{ color: "hsl(354 85% 75%)" }}>
          {syncState.message}
        </p>
      ) : null}

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
          style={{ border: "1px solid hsl(var(--surface-4))", color: "hsl(var(--foreground))" }}
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
                  border: action.done ? "1px solid hsl(var(--primary))" : "1px solid hsl(var(--surface-4))",
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
        manualActions.length === 0 ? (
          <p className="text-[14px] text-muted-foreground">No explicit action items found. Add your own above.</p>
        ) : null
      ) : (
        <div className="space-y-7">
          {actions.map((action, index) => {
            const key = `${index}-${action.title}`;
            const isDone = completed.has(key);
            return (
              <div key={key} className="grid grid-cols-[26px_minmax(0,1fr)_64px] gap-4">
                <button
                  type="button"
                  onClick={() => onToggle(key)}
                  className="mt-1 flex h-[18px] w-[18px] items-center justify-center rounded-[4px]"
                  style={{
                    background: isDone ? "hsl(var(--primary))" : "transparent",
                    border: isDone ? "1px solid hsl(var(--primary))" : "1px solid hsl(var(--surface-4))",
                    color: "hsl(var(--primary-foreground))",
                  }}
                  title={isDone ? "Mark incomplete" : "Mark complete"}
                >
                  {isDone ? <Check size={12} /> : null}
                </button>
                <div className="min-w-0">
                  <p className="text-[15px] font-bold text-foreground">{action.title}</p>
                  <p className="mt-1 max-w-[108ch] text-[13px] leading-6 text-muted-foreground">
                    {action.evidence || [action.assignee, action.due].filter(Boolean).join(" · ") || "No extra detail captured."}
                  </p>
                </div>
                <div className="flex items-start justify-end gap-2">
                  <IconButton label="Open synced Lark task" disabled>
                    <ExternalLink size={14} />
                  </IconButton>
                  <IconButton label="More">
                    <ChevronDown size={14} />
                  </IconButton>
                </div>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

function DecisionsBlock({ decisions }: { decisions: MeetingAiDecision[] }) {
  if (decisions.length === 0) return null;
  return (
    <section className="mt-8">
      <h3 className="text-[21px] font-bold text-foreground">Decisions</h3>
      <div className="mt-4 space-y-3">
        {decisions.map((decision, index) => (
          <div key={`${decision.text}-${index}`} className="flex gap-3 text-[15px] leading-7 text-muted-foreground">
            <span className="mt-0.5 text-[16px]">◆</span>
            <p>
              <span className="font-semibold text-foreground">{decision.text}</span>
              {decision.evidence ? <span className="block text-[13px] text-muted-foreground">{decision.evidence}</span> : null}
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
  can_retry: boolean;
  error: string | null;
  has_transcript: boolean;
  has_intelligence: boolean;
  summary_failed: boolean;
  updated_at_ms: number;
}

// Ordered processing stages shown in the post-meeting progress stepper.
const PROCESSING_STEPS: { key: string; label: string }[] = [
  { key: "queued", label: "Queued" },
  { key: "transcribing", label: "Transcribing" },
  { key: "cleaning", label: "Cleaning" },
  { key: "diarizing", label: "Diarizing" },
  { key: "summarizing", label: "Summarizing" },
  { key: "summarized", label: "Done" },
];

function processingStepIndex(stage: string): number {
  const i = PROCESSING_STEPS.findIndex((s) => s.key === stage);
  if (i >= 0) return i;
  if (stage === "transcribed") return PROCESSING_STEPS.length - 1;
  return 0;
}

/** Post-meeting progress banner: a live stage stepper while a background job
 *  runs, or a failure state with a Retry button. Shown above the detail tabs. */
function ProcessingBanner({
  status,
  onRetryTranscribe,
  onRetrySummary,
  retrying,
}: {
  status: MeetingProcessingStatus;
  onRetryTranscribe: () => void;
  onRetrySummary: () => void;
  retrying: boolean;
}) {
  const summaryFailed = !status.running && status.summary_failed;
  const failed = !status.running && (status.can_retry || summaryFailed);
  const onRetry = summaryFailed ? onRetrySummary : onRetryTranscribe;
  const activeIdx = processingStepIndex(status.stage);
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
          ) : (
            <Loader2 size={16} className="animate-spin" style={{ color: "hsl(var(--primary))" }} />
          )}
          <span className="text-[13px] font-bold text-foreground">
            {summaryFailed
              ? "Summary failed — transcript is saved"
              : failed
                ? "Processing failed"
                : status.queued
                  ? "Queued for processing…"
                  : "Processing your meeting…"}
          </span>
        </div>
        {failed ? (
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
                  {step.label}
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
  /** Open Settings → Meeting (to download/select a transcription model). */
  onConfigureModels?: () => void;
}

export function MeetingsView({
  onJoinMeeting,
  focusMeetingId,
  onFocusConsumed,
  onConfigureModels,
}: MeetingsViewProps) {
  const [meetings, setMeetings] = useState<Meeting[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [creating, setCreating] = useState(false);
  const [dateFilter, setDateFilter] = useState<"all" | "today" | "week" | "archived">("all");
  const [searchQuery, setSearchQuery] = useState("");
  const [searchHits, setSearchHits] = useState<Record<string, MeetingSearchHit> | null>(null);
  const [searchBusy, setSearchBusy] = useState(false);
  const [selectedMeetingId, setSelectedMeetingId] = useState<string | null>(null);
  const [detailTab, setDetailTab] = useState<DetailTab>("summary");
  const [procStatus, setProcStatus] = useState<MeetingProcessingStatus | null>(null);
  const [meetingAi, setMeetingAi] = useState<MeetingIntelligenceResult | null>(null);
  const [meetingAiLoading, setMeetingAiLoading] = useState(false);
  const [meetingAiError, setMeetingAiError] = useState<string | null>(null);
  const [artifacts, setArtifacts] = useState<MeetingCachedArtifacts | null>(null);
  const [artifactsLoading, setArtifactsLoading] = useState(false);
  const [completedActions, setCompletedActions] = useState<Set<string>>(new Set());
  const [manualActions, setManualActions] = useState<ManualAction[]>([]);
  const [actionDraft, setActionDraft] = useState("");
  const [syncState, setSyncState] = useState<SyncState>({ kind: "idle" });
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
  const [audioCurrentTime, setAudioCurrentTime] = useState(0);
  const [audioDuration, setAudioDuration] = useState(0);
  const [audioPlaying, setAudioPlaying] = useState(false);
  const [audioRate, setAudioRate] = useState(1);
  const audioRef = useRef<HTMLAudioElement | null>(null);

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
      const result = (await listMeetings(conn.serverUrl, conn.jwt)) as Meeting[];
      setMeetings(result);
      // Enrich the list with locally-cached AI titles, word counts, and counts
      // in a single batch call so every card reflects its own analysis.
      const ids = result.map((meeting) => meeting.id);
      if (ids.length > 0) {
        invoke<Record<string, MeetingOverview>>("meeting_engine_get_meeting_overviews", {
          meetingIds: ids,
        })
          .then((map) => setOverviews(map))
          .catch(() => {});
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load meetings");
    } finally {
      setLoading(false);
    }
  }, []);

  // Re-read overviews for the current list (used after a local mutation so the
  // backend stays the single source of truth for title/favorite/hidden/tags).
  const refreshOverviews = useCallback(async () => {
    const ids = meetings.map((meeting) => meeting.id);
    if (ids.length === 0) return;
    try {
      const map = await invoke<Record<string, MeetingOverview>>(
        "meeting_engine_get_meeting_overviews",
        { meetingIds: ids },
      );
      setOverviews(map);
    } catch {
      /* best-effort */
    }
  }, [meetings]);

  useEffect(() => {
    void fetchMeetings();
    const interval = setInterval(fetchMeetings, 15_000);
    return () => clearInterval(interval);
  }, [fetchMeetings]);

  // A meeting can't be transcribed without an installed model. Poll the installed
  // model list (null = still checking) so we can block starting + prompt to
  // download. Re-checks every 5s so the banner clears soon after a download.
  const [hasModel, setHasModel] = useState<boolean | null>(null);
  useEffect(() => {
    let cancelled = false;
    const check = async () => {
      try {
        // Keep a model selected whenever one is installed (auto-select single).
        await invoke("meeting_ensure_active_model").catch(() => null);
        const models = await invoke<{ incomplete: boolean }[]>("meeting_list_whisper_models");
        if (!cancelled) setHasModel(models.some((m) => !m.incomplete));
      } catch {
        if (!cancelled) setHasModel(null);
      }
    };
    void check();
    const id = setInterval(check, 5_000);
    return () => {
      cancelled = true;
      clearInterval(id);
    };
  }, []);

  const handleNewMeeting = useCallback(async () => {
    if (hasModel === false) {
      setError("Install a transcription model first (Settings → Meeting).");
      onConfigureModels?.();
      return;
    }
    const conn = getConnection();
    if (!conn) {
      setError("Not connected to enterprise server");
      return;
    }
    setCreating(true);
    setError("");
    try {
      const activeConn = await repairEnterpriseConnection(conn);
      const meeting = await createMeeting(activeConn.serverUrl, activeConn.jwt, {
        title: `Quick meeting ${new Date().toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })}`,
        agenda: null,
        participant_ids: [activeConn.accountId],
        duration_minutes: 30,
      });
      await startMeeting(activeConn.serverUrl, activeConn.jwt, meeting.id);
      await fetchMeetings();
      onJoinMeeting?.(meeting.id);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to create meeting");
    } finally {
      setCreating(false);
    }
  }, [fetchMeetings, onJoinMeeting, hasModel, onConfigureModels]);

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
    // Archived tab: ONLY meetings removed from the list whose files are still on
    // disk (not file-deleted). Other tabs exclude archived meetings.
    if (dateFilter === "archived") {
      if (searching) return Boolean(ov?.hidden && ov?.has_local_files && searchHits?.[meeting.id]);
      return Boolean(ov?.hidden && ov?.has_local_files);
    }
    if (ov?.hidden) return false;
    // When searching, restrict to backend hits and ignore the date filter so
    // matches are never hidden by the All/Today/Week tabs.
    if (searching) return Boolean(searchHits?.[meeting.id]);
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

  // Keep selectedMeetingId authoritative: pick the first meeting when nothing is
  // selected, and re-point it if the current selection was hidden/deleted/filtered
  // out. This guarantees the load and sync effects always agree on one id.
  const firstMeetingId = sortedMeetings[0]?.id ?? null;
  useEffect(() => {
    if (sortedMeetings.length === 0) {
      if (selectedMeetingId !== null) setSelectedMeetingId(null);
      return;
    }
    const stillVisible = sortedMeetings.some((meeting) => meeting.id === selectedMeetingId);
    if (!stillVisible) setSelectedMeetingId(firstMeetingId);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedMeetingId, firstMeetingId, sortedMeetings.length]);

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
  const transcriptWordCount = wordCount(artifacts?.transcript);
  const selectedWordCount = transcriptWordCount || wordCount(meetingAi?.summary) || wordCount(selectedMeeting?.agenda);
  const sectionLabel = sortedMeetings[0]
    ? meetingTime(sortedMeetings[0]).toLocaleDateString(undefined, { month: "short", year: "numeric" }).toUpperCase()
    : "RECENT";

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
    setSyncState({ kind: "idle" });
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

  const copyTranscript = useCallback(async () => {
    const text = artifacts?.transcript?.trim();
    if (!text) return;
    await navigator.clipboard.writeText(text);
  }, [artifacts?.transcript]);

  const handleSyncToLark = useCallback(async () => {
    if (!selectedMeeting) return;
    const summary = meetingAi?.summary?.trim();
    if (!summary) {
      setSyncState({ kind: "error", code: "no_summary", message: "Generate a summary first, then export." });
      return;
    }
    const title =
      overviews[selectedMeeting.id]?.title?.trim()
      || meetingAi?.title?.trim()
      || selectedMeeting.title;
    const payload = {
      title,
      summary,
      action_items: [
        ...(meetingAi?.action_items ?? []).map((item) => ({
          title: item.title,
          assignee: item.assignee ?? null,
        })),
        // Include the user's manually-added actions in the exported doc.
        ...manualActions.map((action) => ({
          title: action.done ? `${action.title} (done)` : action.title,
          assignee: null,
        })),
      ],
      decisions: (meetingAi?.decisions ?? []).map((decision) => decision.text),
    };
    setSyncState({ kind: "syncing" });
    let result = await exportMeetingToLark(selectedMeeting.id, payload);
    // One automatic retry if the session lapsed.
    if (!result.ok && result.code === "unauthorized") {
      const conn = getConnection();
      if (conn) {
        try {
          await repairEnterpriseConnection(conn);
        } catch {
          /* ignore — the retry will surface the real error */
        }
      }
      result = await exportMeetingToLark(selectedMeeting.id, payload);
    }
    if (result.ok) {
      setSyncState({ kind: "done", url: result.url, inSharedFolder: result.inSharedFolder, warning: result.warning });
      try {
        await invoke("meeting_engine_set_meeting_lark_doc", { meetingId: selectedMeeting.id, url: result.url });
        await refreshOverviews();
      } catch {
        /* best-effort idempotency cache */
      }
    } else {
      setSyncState({ kind: "error", code: result.code, message: result.message });
    }
  }, [selectedMeeting, meetingAi, overviews, refreshOverviews, manualActions]);

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
    if (!selectedMeeting || retranscribing) return;
    const id = selectedMeeting.id;
    setRetranscribing(true);
    try {
      await invoke("meeting_engine_retranscribe", { meetingId: id });
      // Poll the engine until the background transcription job finishes.
      const startedAt = Date.now();
      for (;;) {
        await new Promise((resolve) => setTimeout(resolve, 1500));
        if (Date.now() - startedAt > 6 * 60 * 1000) break;
        let running = false;
        try {
          const status = await invoke<{ transcription?: { running?: boolean } }>(
            "meeting_engine_get_status",
          );
          running = Boolean(status?.transcription?.running);
        } catch {
          break;
        }
        if (!running) break;
      }
      // Reload artifacts, but only if this meeting is still selected.
      const fresh = await invoke<MeetingCachedArtifacts | null>("meeting_engine_get_cached_artifacts", {
        meetingId: id,
      });
      if (selectedIdRef.current === id) setArtifacts(fresh);
      await refreshOverviews();
    } catch (err) {
      console.warn("[meeting] re-transcribe failed:", err);
    } finally {
      setRetranscribing(false);
    }
  }, [selectedMeeting, retranscribing, refreshOverviews]);

  // Poll per-meeting processing status so the post-meeting stages (transcribing
  // → cleaning → diarizing → ready) render live, and reload artifacts the moment
  // a background job finishes. Polls every 2s only while a job is active, so an
  // idle/finished meeting costs a single call.
  const procWasRunningRef = useRef(false);
  useEffect(() => {
    const id = selectedMeeting?.id;
    if (!id) {
      setProcStatus(null);
      procWasRunningRef.current = false;
      return;
    }
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
      setProcStatus(status);
      const running = Boolean(status?.running);
      // running → finished: reload the freshly-written transcript AND summary
      // (the worker now auto-generates the summary as the final stage).
      if (procWasRunningRef.current && !running) {
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
      procWasRunningRef.current = running;
      if (running && !cancelled) {
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
  const larkDocUrl = selectedMeeting
    ? (syncState.kind === "done" ? syncState.url : null) || overviews[selectedMeeting.id]?.lark_doc_url || null
    : null;

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

  const toggleFavorite = useCallback(async () => {
    if (!selectedMeeting) return;
    try {
      await invoke("meeting_engine_set_meeting_favorite", {
        meetingId: selectedMeeting.id,
        favorite: !isFavorite,
      });
      await refreshOverviews();
    } catch (err) {
      console.warn("[meeting] set favorite failed:", err);
    }
  }, [selectedMeeting, isFavorite, refreshOverviews]);

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
      await invoke("meeting_engine_delete_meeting_files", { meetingId: id });
      await refreshOverviews();
    } catch (err) {
      console.warn("[meeting] delete files failed:", err);
    }
  }, [pendingDelete, refreshOverviews]);

  const detailTabs: Array<{ id: DetailTab; label: string; icon: ReactNode }> = [
    { id: "summary", label: "Summary", icon: <Sparkles size={15} /> },
    { id: "notes", label: "My Notes", icon: <FileText size={15} /> },
    { id: "transcript", label: "Transcript", icon: <ScrollText size={15} /> },
    { id: "actions", label: `Actions ${selectedActionCount}`, icon: <ListChecks size={15} /> },
    { id: "chat", label: "AI Chat", icon: <MessageSquare size={15} /> },
  ];

  return (
    <div className="flex h-full flex-col overflow-hidden">
      {hasModel === false ? (
        <div
          className="flex flex-wrap items-center gap-3 px-5 py-2.5"
          style={{ background: "hsl(38 70% 13%)", borderBottom: "1px solid hsl(38 60% 30%)" }}
        >
          <AlertTriangle size={15} className="flex-shrink-0" style={{ color: "hsl(38 92% 66%)" }} />
          <span className="min-w-0 flex-1 text-[12px] text-foreground">
            <span className="font-semibold">No transcription model installed.</span> Meetings can't
            be transcribed until you download and select a model.
          </span>
          <button
            type="button"
            onClick={() => onConfigureModels?.()}
            className="h-7 flex-shrink-0 rounded-lg px-3 text-[12px] font-bold"
            style={{ background: "hsl(38 92% 60%)", color: "hsl(38 92% 10%)" }}
          >
            Download a model
          </button>
        </div>
      ) : null}
      <div className="relative flex min-h-0 flex-1 overflow-hidden">
      <aside className="flex w-[240px] flex-shrink-0 flex-col xl:w-[330px]" style={{ borderRight: "1px solid hsl(var(--surface-4))" }}>
        <div className="px-4 pb-3 pt-5">
          <div className="flex items-center justify-between">
            <div>
              <h1 className="text-[18px] font-bold text-foreground">Meetings</h1>
              <p className="text-[11px] text-muted-foreground">{liveCount} live · {endedCount} ended</p>
            </div>
            <div className="flex items-center gap-1.5">
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
          <div className="mt-3 flex h-9 items-center gap-2 rounded-lg px-3" style={{ background: "hsl(var(--surface-3))", border: "1px solid hsl(var(--surface-4))" }}>
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
          <div className="mt-3 flex items-center gap-1.5">
            {[
              { id: "all" as const, label: "All" },
              { id: "today" as const, label: "Today" },
              { id: "week" as const, label: "This Week" },
              { id: "archived" as const, label: "Archived" },
            ].map((filter) => (
              <button
                key={filter.id}
                type="button"
                onClick={() => setDateFilter(filter.id)}
                className="h-7 rounded-lg px-2.5 text-[11px] font-semibold"
                style={{
                  background: dateFilter === filter.id ? "hsl(var(--surface-4))" : "transparent",
                  color: dateFilter === filter.id ? "hsl(var(--foreground))" : "hsl(var(--muted-foreground))",
                }}
              >
                {filter.label}
              </button>
            ))}
          </div>
        </div>

        <div className="flex-1 overflow-y-auto px-3 pb-4">
          {loading && meetings.length === 0 ? (
            <div className="flex h-full flex-col items-center justify-center gap-3 opacity-60">
              <Loader2 size={20} className="animate-spin text-muted-foreground" />
              <p className="text-[12px] text-muted-foreground">Loading meetings...</p>
            </div>
          ) : error ? (
            <div className="flex h-full flex-col items-center justify-center gap-3">
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
            <div className="flex h-full flex-col items-center justify-center gap-3 opacity-60">
              <Video size={28} className="text-muted-foreground" />
              <p className="text-[12px] text-muted-foreground">
                {searching
                  ? searchBusy
                    ? "Searching…"
                    : `No meetings match “${searchQuery.trim()}”`
                  : "No meetings for this filter"}
              </p>
            </div>
          ) : (
            <div className="space-y-2">
              <p className="px-1 pb-1 text-[10px] font-bold uppercase tracking-[0.14em] text-muted-foreground">
                {searching ? `${sortedMeetings.length} result${sortedMeetings.length === 1 ? "" : "s"}` : sectionLabel}
              </p>
              {sortedMeetings.map((meeting) => (
                <MeetingCard
                  key={meeting.id}
                  meeting={meeting}
                  overview={overviews[meeting.id]}
                  searchHit={searching ? searchHits?.[meeting.id] : undefined}
                  selected={meeting.id === selectedMeeting?.id}
                  onSelect={() => {
                    setSelectedMeetingId(meeting.id);
                    setDetailTab("summary");
                  }}
                />
              ))}
            </div>
          )}
        </div>
      </aside>

      <main className="min-w-0 flex-1 overflow-y-auto px-4 pb-12 pt-6 lg:px-10">
        {pendingDelete ? (
          <div
            className="mx-auto mb-5 flex w-full max-w-[1280px] flex-wrap items-center gap-x-4 gap-y-2 rounded-xl px-4 py-2.5"
            style={{ background: "hsl(var(--surface-2))", border: "1px solid hsl(var(--surface-4))" }}
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
        {selectedMeeting ? (
          <div className="mx-auto w-full max-w-[1280px]">
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
                    className="w-full bg-transparent text-[30px] font-bold text-foreground outline-none"
                    style={{ borderBottom: "2px solid hsl(var(--primary) / 0.6)" }}
                  />
                ) : (
                  <h2 className="truncate text-[30px] font-bold text-foreground">{displayTitle}</h2>
                )}
                <div className="mt-4 flex flex-wrap items-center gap-3 text-[13px] font-semibold text-muted-foreground">
                  <span>{formatMeetingDate(selectedMeeting)}</span>
                  <span>·</span>
                  <span>{formatTimestamp((artifacts?.audio_duration_ms ?? audioDuration * 1000) || 0)}</span>
                  <span>·</span>
                  <span>{selectedMeeting.status === "live" ? "Live" : "Local"}</span>
                  <span>·</span>
                  <span>{selectedMeeting.participants_count ?? 0}p</span>
                  <span>·</span>
                  <span>{selectedWordCount} words</span>
                  {meetingAi?.model ? (
                    <>
                      <span>·</span>
                      <span>{meetingAi.transcript_source} · {meetingAi.model}</span>
                    </>
                  ) : null}
                </div>
                <div className="mt-5 flex flex-wrap items-center gap-2">
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
                      className="h-7 w-32 rounded-lg bg-transparent px-2.5 text-[12px] font-bold outline-none"
                      style={{ border: "1px solid hsl(var(--primary) / 0.5)", color: "hsl(var(--foreground))" }}
                    />
                  ) : (
                    <button
                      type="button"
                      onClick={() => {
                        setTagDraft("");
                        setAddingTag(true);
                      }}
                      className="rounded-lg border border-dashed px-3 py-1 text-[12px] font-bold text-muted-foreground transition-colors hover:text-foreground"
                      style={{ borderColor: "hsl(var(--surface-4))" }}
                    >
                      + Add
                    </button>
                  )}
                  {tags.map((tag) => (
                    <span
                      key={`${tag.source}-${tag.label}`}
                      className="group inline-flex items-center gap-1 rounded-md px-2.5 py-1 text-[12px] font-bold"
                      style={{ background: `${tag.color}22`, color: tag.color }}
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

            <div className="mt-5 flex flex-wrap items-center gap-3">
              <ToolbarButton
                icon={syncState.kind === "syncing" ? <Loader2 size={15} className="animate-spin" /> : <FileText size={15} />}
                label={
                  syncState.kind === "syncing"
                    ? "Exporting to Lark…"
                    : larkDocUrl
                      ? "Re-export to Lark"
                      : "Export to Lark Docs"
                }
                active={syncState.kind === "done"}
                disabled={syncState.kind === "syncing" || !meetingAi?.summary}
                onClick={handleSyncToLark}
              />
              {larkDocUrl ? (
                <button
                  type="button"
                  onClick={() => void openExternal(larkDocUrl)}
                  className="flex h-10 items-center gap-2 rounded-lg px-3 text-[12px] font-bold"
                  style={{ background: "hsl(132 38% 12%)", color: "hsl(132 72% 62%)", border: "1px solid hsl(132 56% 32%)" }}
                >
                  <ExternalLink size={14} />
                  Open in Lark
                </button>
              ) : null}
              {syncState.kind === "done" ? (
                <span className="text-[12px] font-semibold" style={{ color: "hsl(132 72% 62%)" }}>
                  Exported{syncState.inSharedFolder ? " to shared folder" : ""}
                  {syncState.warning ? " · content partial (see Lark)" : ""}
                </span>
              ) : syncState.kind === "error" ? (
                <span className="text-[12px] font-semibold" style={{ color: "hsl(354 85% 75%)" }}>
                  {syncState.message}
                </span>
              ) : !meetingAi?.summary ? (
                <span className="text-[12px] text-muted-foreground">Generate a summary first to export.</span>
              ) : null}
            </div>

            {procStatus && (procStatus.running || procStatus.can_retry || procStatus.summary_failed) ? (
              <ProcessingBanner
                status={procStatus}
                onRetryTranscribe={handleRetranscribe}
                onRetrySummary={handleReanalyze}
                retrying={retranscribing || meetingAiLoading}
              />
            ) : null}

            <div className="mt-6 grid grid-cols-5 border-b" style={{ borderColor: "hsl(var(--surface-4))" }}>
              {detailTabs.map((tab) => (
                <button
                  key={tab.id}
                  type="button"
                  onClick={() => setDetailTab(tab.id)}
                  title={tab.label}
                  className="flex h-14 min-w-0 items-center justify-center gap-1.5 px-1 text-[12px] font-bold lg:gap-2 lg:text-[14px]"
                  style={{
                    color: detailTab === tab.id ? "hsl(var(--foreground))" : "hsl(var(--muted-foreground))",
                    borderBottom: detailTab === tab.id ? "2px solid hsl(var(--primary))" : "2px solid transparent",
                  }}
                >
                  <span className="flex-shrink-0">{tab.icon}</span>
                  <span className="truncate">{tab.label}</span>
                </button>
              ))}
            </div>

            {detailTab === "summary" ? (
              <div className="pt-8">
                <div className="mb-5 flex items-center justify-between gap-3">
                  <h3 className="text-[15px] font-bold text-foreground">Summary</h3>
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
                    <ToolbarButton icon={<Copy size={14} />} label="Copy" disabled={!meetingAi?.summary} onClick={copySummary} />
                  </div>
                </div>
                {meetingAiLoading ? (
                  <div className="flex items-center gap-3 text-[14px] text-muted-foreground">
                    <Loader2 size={16} className="animate-spin" />
                    Loading generated summary
                  </div>
                ) : meetingAiError ? (
                  <p className="text-[14px]" style={{ color: "hsl(354 85% 75%)" }}>{meetingAiError}</p>
                ) : meetingAi?.summary?.trim() ? (
                  <>
                    <div className="rounded-xl px-7 py-6" style={{ background: "hsl(var(--surface-2))", border: "1px solid hsl(var(--surface-4))" }}>
                      <div className="flex gap-4">
                        <Sparkles size={19} style={{ color: "hsl(var(--primary))" }} />
                        <p className="max-w-[110ch] text-[17px] italic leading-8 text-muted-foreground">
                          {stripInlineMarkdown(summaryLead(meetingAi.summary))}
                        </p>
                      </div>
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
              <div className="pt-8">
                <div className="mb-5 flex items-center justify-between gap-3">
                  <div className="flex items-center gap-3">
                    <h3 className="text-[15px] font-bold text-foreground">My Notes</h3>
                    <span className="text-[11px] font-semibold text-muted-foreground">
                      {notesStatus === "saving" ? "Saving…" : notesStatus === "saved" ? "Saved ✓" : ""}
                    </span>
                  </div>
                  {notes.trim() ? (
                    <div className="flex items-center gap-1 rounded-lg p-0.5" style={{ background: "hsl(var(--surface-2))", border: "1px solid hsl(var(--surface-4))" }}>
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
                  <div className="rounded-xl px-7 py-6" style={{ background: "hsl(var(--surface-2))", border: "1px solid hsl(var(--surface-4))" }}>
                    <MeetingRichText text={notes} />
                  </div>
                ) : (
                  <div className="rounded-xl" style={{ background: "hsl(var(--surface-2))", border: "1px solid hsl(var(--surface-4))" }}>
                    <textarea
                      value={notes}
                      onChange={(event) => handleNotesChange(event.currentTarget.value)}
                      placeholder="Jot down anything from this meeting — decisions, follow-ups, your own takeaways. Markdown supported. These notes are private to you and are used as extra context when you ask the AI Chat about this meeting."
                      className="min-h-[340px] w-full resize-y rounded-xl bg-transparent px-7 py-6 text-[15px] leading-8 text-foreground outline-none placeholder:text-muted-foreground"
                      spellCheck
                    />
                  </div>
                )}
                <p className="mt-3 flex items-center gap-2 text-[12px] text-muted-foreground">
                  <Sparkles size={13} style={{ color: "hsl(var(--primary))" }} />
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
                retranscribing={retranscribing}
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
                onSync={handleSyncToLark}
                syncState={syncState}
                manualActions={manualActions}
                actionDraft={actionDraft}
                onActionDraftChange={setActionDraft}
                onAddManualAction={handleAddManualAction}
                onToggleManualAction={handleToggleManualAction}
                onRemoveManualAction={handleRemoveManualAction}
              />
            ) : null}

            {detailTab === "chat" ? (
              <div className="pt-8">
                <h3 className="mb-5 text-[15px] font-bold text-foreground">AI Chat</h3>
                <div
                  className="h-[560px] overflow-hidden rounded-xl"
                  style={{ background: "hsl(var(--surface-2))", border: "1px solid hsl(var(--surface-4))" }}
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
        ) : (
          <div className="flex h-full flex-col items-center justify-center gap-3 text-center opacity-70">
            <Video size={28} className="text-muted-foreground" />
            <div>
              <p className="text-[14px] font-semibold text-foreground">Open a meeting note</p>
              <p className="mt-1 text-[12px] text-muted-foreground">
                Click any meeting card to open Summary, My Notes, Transcript, Actions, and AI Chat here.
              </p>
            </div>
          </div>
        )}
      </main>
      </div>
    </div>
  );
}
