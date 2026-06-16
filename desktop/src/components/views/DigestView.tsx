import { useCallback, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  AlertTriangle,
  CalendarRange,
  Check,
  CheckSquare,
  Copy,
  ExternalLink,
  Layers,
  ListChecks,
  Loader2,
  RefreshCw,
  Sparkles,
  Square,
} from "lucide-react";
import { MeetingAiChat } from "@/components/MeetingAiChat";
import { MeetingRichText } from "@/lib/meetingMarkdown";
import { ensureActiveWorkspace, exportMeetingToLark, getConnection } from "@/lib/enterprise";
import { openExternal } from "@/lib/invoke";

// Subsets of the MeetingsView types — passed in by structural typing.
interface DigestMeetingInput {
  id: string;
  title: string;
  status: "scheduled" | "live" | "ended";
  scheduled_at?: string | null;
  created_at: string;
}

interface DigestOverview {
  title?: string | null;
  has_intelligence: boolean;
}

interface DigestResult {
  id: string;
  title: string;
  date_range: string;
  meeting_count: number;
  included_meeting_ids: string[];
  skipped: { id: string; title: string; reason: string }[];
  executive_summary: string;
  themes: { title: string; detail: string; meetings: string[] }[];
  decisions: { text: string; meeting: string; date: string }[];
  action_items: { title: string; owner?: string | null; meeting: string; date: string }[];
  trends: string[];
  open_items: string[];
  per_meeting: { id: string; title: string; date: string; recap: string; has_intelligence: boolean }[];
  markdown: string;
  provider: string;
  model: string;
  latency_ms: number;
}

type MissingStrategy = "skip" | "generate";
type SelectMode = "select" | "range";
type RangePreset = "7d" | "30d" | "month" | "custom";

interface DigestViewProps {
  meetings: DigestMeetingInput[];
  overviews: Record<string, DigestOverview>;
}

function meetingTimeMs(m: DigestMeetingInput): number {
  const t = new Date(m.scheduled_at ?? m.created_at).getTime();
  return Number.isFinite(t) ? t : 0;
}

function formatDate(m: DigestMeetingInput): string {
  try {
    return new Date(m.scheduled_at ?? m.created_at).toLocaleDateString(undefined, {
      day: "numeric",
      month: "short",
      year: "numeric",
    });
  } catch {
    return "";
  }
}

function meetingTitle(m: DigestMeetingInput, overviews: Record<string, DigestOverview>): string {
  return overviews[m.id]?.title?.trim() || m.title || "Untitled meeting";
}

export function DigestView({ meetings, overviews }: DigestViewProps) {
  const [selectMode, setSelectMode] = useState<SelectMode>("select");
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const [rangePreset, setRangePreset] = useState<RangePreset>("7d");
  const [customFrom, setCustomFrom] = useState("");
  const [customTo, setCustomTo] = useState("");

  const [digest, setDigest] = useState<DigestResult | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [preflight, setPreflight] = useState<{ total: number; missing: number } | null>(null);

  const [copied, setCopied] = useState(false);
  const [larkBusy, setLarkBusy] = useState(false);
  const [larkError, setLarkError] = useState<string | null>(null);
  const [larkReauth, setLarkReauth] = useState(false);
  const [larkUrl, setLarkUrl] = useState<string | null>(null);

  // Only ended meetings can feed a digest.
  const eligible = useMemo(
    () => meetings.filter((m) => m.status === "ended").sort((a, b) => meetingTimeMs(b) - meetingTimeMs(a)),
    [meetings],
  );

  // The meetings implied by the active date range.
  const rangeMeetings = useMemo(() => {
    if (selectMode !== "range") return [];
    const now = Date.now();
    let from = 0;
    let to = now;
    if (rangePreset === "7d") from = now - 7 * 86_400_000;
    else if (rangePreset === "30d") from = now - 30 * 86_400_000;
    else if (rangePreset === "month") {
      const d = new Date();
      from = new Date(d.getFullYear(), d.getMonth(), 1).getTime();
    } else {
      from = customFrom ? new Date(customFrom).getTime() : 0;
      to = customTo ? new Date(customTo).getTime() + 86_400_000 - 1 : now;
    }
    return eligible.filter((m) => {
      const t = meetingTimeMs(m);
      return t >= from && t <= to;
    });
  }, [selectMode, rangePreset, customFrom, customTo, eligible]);

  // The effective selection (chronological), and the refs sent to the backend.
  const selectedMeetings = useMemo(() => {
    const base = selectMode === "select" ? eligible.filter((m) => selectedIds.has(m.id)) : rangeMeetings;
    return base.slice().sort((a, b) => meetingTimeMs(a) - meetingTimeMs(b));
  }, [selectMode, eligible, selectedIds, rangeMeetings]);

  const refs = useMemo(
    () =>
      selectedMeetings.map((m) => ({
        id: m.id,
        title: meetingTitle(m, overviews),
        date: formatDate(m),
      })),
    [selectedMeetings, overviews],
  );

  const missingCount = useMemo(
    () => selectedMeetings.filter((m) => !overviews[m.id]?.has_intelligence).length,
    [selectedMeetings, overviews],
  );

  const runGenerate = useCallback(
    async (missing: MissingStrategy) => {
      setPreflight(null);
      setLoading(true);
      setError(null);
      setDigest(null);
      setLarkError(null);
      setLarkReauth(false);
      setLarkUrl(null);
      try {
        const result = await invoke<DigestResult>("meeting_engine_generate_digest", { refs, missing });
        setDigest(result);
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
      } finally {
        setLoading(false);
      }
    },
    [refs],
  );

  const onGenerate = useCallback(() => {
    if (refs.length === 0) {
      setError("Select at least one meeting (or a date range with meetings) to build a digest.");
      return;
    }
    setError(null);
    if (missingCount > 0) {
      setPreflight({ total: refs.length, missing: missingCount });
      return;
    }
    void runGenerate("skip");
  }, [refs, missingCount, runGenerate]);

  const onCopy = useCallback(async () => {
    if (!digest) return;
    await navigator.clipboard.writeText(digest.markdown);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  }, [digest]);

  // Lark export reuses the single-meeting endpoint, so it needs a real cloud
  // meeting id in the path — pick the first cloud-synced meeting in the set.
  const firstCloudId = useMemo(
    () => digest?.included_meeting_ids.find((id) => !id.startsWith("local-")) ?? null,
    [digest],
  );

  const onExportLark = useCallback(async () => {
    if (!digest) return;
    if (!firstCloudId) {
      setLarkError("Lark export needs at least one cloud-synced meeting in the selection.");
      return;
    }
    setLarkBusy(true);
    setLarkError(null);
    setLarkReauth(false);
    try {
      if (!(await ensureActiveWorkspace())) {
        setLarkError("Pick a workspace first, then export.");
        return;
      }
      const result = await exportMeetingToLark(firstCloudId, {
        title: digest.title,
        summary: digest.markdown,
        action_items: digest.action_items.map((a) => ({ title: a.title, assignee: a.owner ?? null })),
        decisions: digest.decisions.map((d) => d.text).filter(Boolean),
      });
      if (result.ok) {
        setLarkUrl(result.url);
        if (result.url) void openExternal(result.url);
      } else {
        setLarkError(result.message);
        setLarkReauth(
          result.code === "lark_reauth_required" ||
            result.code === "lark_not_linked" ||
            result.code === "unauthorized",
        );
      }
    } finally {
      setLarkBusy(false);
    }
  }, [digest, firstCloudId]);

  const toggleId = useCallback((id: string) => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }, []);

  const chatRefs = useMemo(
    () => digest?.per_meeting.map((m) => ({ id: m.id, title: m.title, date: m.date })) ?? [],
    [digest],
  );

  const hasConnection = Boolean(getConnection());

  return (
    <div className="relative flex min-h-0 flex-1 overflow-hidden">
      {/* Selection panel */}
      <aside
        className="flex w-[280px] flex-shrink-0 flex-col xl:w-[340px]"
        style={{ borderRight: "1px solid hsl(var(--surface-4))" }}
      >
        <div className="px-4 pb-3 pt-5">
          <h1 className="flex items-center gap-2 text-[18px] font-bold text-foreground">
            <Layers size={18} /> Digest
          </h1>
          <p className="text-[11px] text-muted-foreground">
            Combine meetings into one smart report you can chat with.
          </p>
        </div>

        {/* Select vs date-range toggle */}
        <div className="flex gap-1 px-4 pb-3">
          {(
            [
              { id: "select" as const, label: "Select", icon: <CheckSquare size={13} /> },
              { id: "range" as const, label: "Date range", icon: <CalendarRange size={13} /> },
            ]
          ).map((t) => (
            <button
              key={t.id}
              type="button"
              onClick={() => setSelectMode(t.id)}
              className="flex h-8 flex-1 items-center justify-center gap-1.5 rounded-lg text-[12px] font-bold"
              style={{
                background: selectMode === t.id ? "hsl(var(--surface-4))" : "transparent",
                color: selectMode === t.id ? "hsl(var(--foreground))" : "hsl(var(--muted-foreground))",
              }}
            >
              {t.icon}
              {t.label}
            </button>
          ))}
        </div>

        {selectMode === "range" ? (
          <div className="px-4 pb-2">
            <div className="flex flex-wrap gap-1.5">
              {(
                [
                  { id: "7d" as const, label: "Last 7 days" },
                  { id: "30d" as const, label: "Last 30 days" },
                  { id: "month" as const, label: "This month" },
                  { id: "custom" as const, label: "Custom" },
                ]
              ).map((p) => (
                <button
                  key={p.id}
                  type="button"
                  onClick={() => setRangePreset(p.id)}
                  className="h-7 rounded-lg px-2.5 text-[11px] font-semibold"
                  style={{
                    background: rangePreset === p.id ? "hsl(var(--surface-4))" : "transparent",
                    color: rangePreset === p.id ? "hsl(var(--foreground))" : "hsl(var(--muted-foreground))",
                    border: "1px solid hsl(var(--surface-4))",
                  }}
                >
                  {p.label}
                </button>
              ))}
            </div>
            {rangePreset === "custom" ? (
              <div className="mt-2 flex items-center gap-2">
                <input
                  type="date"
                  value={customFrom}
                  onChange={(e) => setCustomFrom(e.currentTarget.value)}
                  className="h-8 min-w-0 flex-1 rounded-lg bg-transparent px-2 text-[12px] outline-none"
                  style={{ border: "1px solid hsl(var(--surface-4))", color: "hsl(var(--foreground))" }}
                />
                <span className="text-[11px] text-muted-foreground">to</span>
                <input
                  type="date"
                  value={customTo}
                  onChange={(e) => setCustomTo(e.currentTarget.value)}
                  className="h-8 min-w-0 flex-1 rounded-lg bg-transparent px-2 text-[12px] outline-none"
                  style={{ border: "1px solid hsl(var(--surface-4))", color: "hsl(var(--foreground))" }}
                />
              </div>
            ) : null}
          </div>
        ) : (
          <div className="flex items-center justify-between px-4 pb-2">
            <span className="text-[11px] text-muted-foreground">{selectedIds.size} selected</span>
            <div className="flex gap-2">
              <button
                type="button"
                onClick={() => setSelectedIds(new Set(eligible.map((m) => m.id)))}
                className="text-[11px] font-semibold text-muted-foreground hover:text-foreground"
              >
                Select all
              </button>
              <button
                type="button"
                onClick={() => setSelectedIds(new Set())}
                className="text-[11px] font-semibold text-muted-foreground hover:text-foreground"
              >
                Clear
              </button>
            </div>
          </div>
        )}

        {/* Eligible meeting list */}
        <div className="min-h-0 flex-1 overflow-y-auto px-3 pb-3">
          {eligible.length === 0 ? (
            <p className="px-1 pt-4 text-[12px] text-muted-foreground">No ended meetings yet.</p>
          ) : (
            eligible.map((m) => {
              const inRange = selectMode === "range" && rangeMeetings.some((r) => r.id === m.id);
              const checked = selectMode === "select" ? selectedIds.has(m.id) : inRange;
              const analyzed = overviews[m.id]?.has_intelligence ?? false;
              return (
                <button
                  key={m.id}
                  type="button"
                  disabled={selectMode === "range"}
                  onClick={() => selectMode === "select" && toggleId(m.id)}
                  className="mb-1 flex w-full items-start gap-2.5 rounded-lg px-2.5 py-2 text-left transition-colors disabled:cursor-default"
                  style={{
                    background: checked ? "hsl(var(--primary) / 0.12)" : "transparent",
                    border: checked ? "1px solid hsl(var(--primary) / 0.4)" : "1px solid transparent",
                    opacity: selectMode === "range" && !inRange ? 0.4 : 1,
                  }}
                >
                  <span className="mt-0.5 flex-shrink-0" style={{ color: checked ? "hsl(var(--primary))" : "hsl(var(--muted-foreground))" }}>
                    {checked ? <CheckSquare size={15} /> : <Square size={15} />}
                  </span>
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-[13px] font-semibold text-foreground">
                      {meetingTitle(m, overviews)}
                    </span>
                    <span className="flex items-center gap-1.5 text-[11px] text-muted-foreground">
                      {formatDate(m)}
                      {!analyzed ? (
                        <span style={{ color: "hsl(38 90% 66%)" }}>· not analyzed</span>
                      ) : null}
                    </span>
                  </span>
                </button>
              );
            })
          )}
        </div>

        {/* Generate */}
        <div className="border-t p-3" style={{ borderColor: "hsl(var(--surface-4))" }}>
          {preflight ? (
            <div
              className="mb-2 rounded-lg p-3 text-[12px]"
              style={{ background: "hsl(38 70% 13%)", border: "1px solid hsl(38 60% 30%)" }}
            >
              <p className="flex items-start gap-1.5 text-foreground">
                <AlertTriangle size={14} className="mt-0.5 flex-shrink-0" style={{ color: "hsl(38 92% 66%)" }} />
                <span>
                  {preflight.missing} of {preflight.total} selected meeting{preflight.total === 1 ? "" : "s"}{" "}
                  {preflight.missing === 1 ? "isn't" : "aren't"} analyzed yet.
                </span>
              </p>
              <div className="mt-2.5 flex gap-2">
                <button
                  type="button"
                  onClick={() => void runGenerate("generate")}
                  className="h-8 flex-1 rounded-lg px-2 text-[12px] font-bold"
                  style={{ background: "hsl(38 92% 60%)", color: "hsl(38 92% 10%)" }}
                >
                  Generate them first
                </button>
                <button
                  type="button"
                  onClick={() => void runGenerate("skip")}
                  className="h-8 flex-1 rounded-lg px-2 text-[12px] font-bold"
                  style={{ background: "hsl(var(--surface-4))", color: "hsl(var(--foreground))" }}
                >
                  Skip them
                </button>
              </div>
            </div>
          ) : null}
          <button
            type="button"
            onClick={onGenerate}
            disabled={loading || refs.length === 0}
            className="flex h-10 w-full items-center justify-center gap-2 rounded-lg text-[13px] font-bold disabled:opacity-45"
            style={{ background: "hsl(var(--primary))", color: "hsl(var(--primary-foreground))" }}
          >
            {loading ? (
              <>
                <Loader2 size={15} className="animate-spin" /> Generating…
              </>
            ) : (
              <>
                <Sparkles size={15} /> Generate digest ({refs.length})
              </>
            )}
          </button>
        </div>
      </aside>

      {/* Report + chat */}
      <main className="flex min-w-0 flex-1 flex-col overflow-hidden">
        {!hasConnection ? (
          <div className="flex flex-1 items-center justify-center p-10">
            <p className="max-w-sm text-center text-[13px] text-muted-foreground">
              Connect a workspace to build cross-meeting digests.
            </p>
          </div>
        ) : loading ? (
          <div className="flex flex-1 flex-col items-center justify-center gap-3 p-10">
            <Loader2 size={28} className="animate-spin" style={{ color: "hsl(var(--primary))" }} />
            <p className="text-[13px] text-muted-foreground">
              Synthesizing {refs.length} meeting{refs.length === 1 ? "" : "s"}…
            </p>
          </div>
        ) : error ? (
          <div className="flex flex-1 items-center justify-center p-10">
            <p className="max-w-md text-center text-[13px]" style={{ color: "hsl(354 85% 75%)" }}>
              {error}
            </p>
          </div>
        ) : !digest ? (
          <div className="flex flex-1 flex-col items-center justify-center gap-2 p-10 text-center">
            <Layers size={30} className="text-muted-foreground" />
            <p className="text-[15px] font-bold text-foreground">Build a cross-meeting digest</p>
            <p className="max-w-sm text-[13px] text-muted-foreground">
              Pick meetings or a date range on the left, then generate one combined report — themes,
              decisions, action items — and chat across all of them.
            </p>
          </div>
        ) : (
          <div className="flex min-h-0 flex-1 flex-col overflow-hidden">
            {/* Header + actions */}
            <div className="flex flex-wrap items-start justify-between gap-3 px-6 pb-3 pt-6 lg:px-10">
              <div className="min-w-0">
                <h2 className="text-[22px] font-bold text-foreground">{digest.title}</h2>
                <p className="text-[12px] text-muted-foreground">
                  {digest.date_range ? `${digest.date_range} · ` : ""}
                  {digest.meeting_count} meeting{digest.meeting_count === 1 ? "" : "s"}
                  {digest.skipped.length > 0 ? ` · ${digest.skipped.length} skipped` : ""}
                  {` · ${digest.model}`}
                </p>
              </div>
              <div className="flex flex-shrink-0 items-center gap-2">
                <button
                  type="button"
                  onClick={() => void onCopy()}
                  className="flex h-8 items-center gap-1.5 rounded-lg px-3 text-[12px] font-bold"
                  style={{ background: "hsl(var(--surface-3))", color: "hsl(var(--foreground))", border: "1px solid hsl(var(--surface-4))" }}
                >
                  {copied ? <Check size={14} /> : <Copy size={14} />}
                  {copied ? "Copied" : "Copy"}
                </button>
                {larkUrl ? (
                  <button
                    type="button"
                    onClick={() => void openExternal(larkUrl)}
                    className="flex h-8 items-center gap-1.5 rounded-lg px-3 text-[12px] font-bold"
                    style={{ background: "hsl(var(--surface-3))", color: "hsl(var(--foreground))", border: "1px solid hsl(var(--surface-4))" }}
                  >
                    <ExternalLink size={14} /> Open in Lark
                  </button>
                ) : larkReauth ? (
                  <button
                    type="button"
                    onClick={() => void onExportLark()}
                    disabled={larkBusy}
                    className="flex h-8 items-center gap-1.5 rounded-lg px-3 text-[12px] font-bold"
                    style={{ background: "hsl(38 92% 60%)", color: "hsl(38 92% 10%)" }}
                  >
                    {larkBusy ? <Loader2 size={14} className="animate-spin" /> : <RefreshCw size={14} />}
                    Reconnect Lark
                  </button>
                ) : (
                  <button
                    type="button"
                    onClick={() => void onExportLark()}
                    disabled={larkBusy}
                    className="flex h-8 items-center gap-1.5 rounded-lg px-3 text-[12px] font-bold disabled:opacity-45"
                    style={{ background: "hsl(var(--primary))", color: "hsl(var(--primary-foreground))" }}
                  >
                    {larkBusy ? <Loader2 size={14} className="animate-spin" /> : <ExternalLink size={14} />}
                    Export to Lark
                  </button>
                )}
              </div>
            </div>
            {larkError ? (
              <p className="px-6 pb-2 text-[12px] lg:px-10" style={{ color: "hsl(354 85% 75%)" }}>
                {larkError}
              </p>
            ) : null}

            {/* Body: report + chat side by side on wide screens */}
            <div className="flex min-h-0 flex-1 flex-col overflow-hidden xl:flex-row">
              <div className="min-h-0 flex-1 overflow-y-auto px-6 pb-12 lg:px-10">
                <MeetingRichText text={digest.markdown} />
              </div>
              <div
                className="flex h-[340px] flex-shrink-0 flex-col xl:h-auto xl:w-[420px]"
                style={{ borderTop: "1px solid hsl(var(--surface-4))" }}
              >
                <div
                  className="flex items-center gap-1.5 px-4 py-2.5 text-[12px] font-bold text-foreground xl:border-l"
                  style={{ borderColor: "hsl(var(--surface-4))", background: "hsl(var(--surface-2))" }}
                >
                  <ListChecks size={14} /> Ask across these meetings
                </div>
                <div className="min-h-0 flex-1 xl:border-l" style={{ borderColor: "hsl(var(--surface-4))" }}>
                  <MeetingAiChat
                    resetKey={digest.id}
                    summary={digest.executive_summary || null}
                    transcriptOverride={null}
                    canSend
                    chatCommand="meeting_engine_digest_chat"
                    chatArgs={{ refs: chatRefs, digestSummary: digest.executive_summary }}
                    emptyHint="Ask across these meetings — compare decisions, find an owner, track a topic over time. Answers cite the source meeting."
                    placeholder="Ask about these meetings…"
                  />
                </div>
              </div>
            </div>
          </div>
        )}
      </main>
    </div>
  );
}
