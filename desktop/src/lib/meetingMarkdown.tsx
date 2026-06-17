import type { ReactNode } from "react";

// Shared markdown parsing + rendering for meeting content (summary tab, post-
// meeting chat, and live-meeting chat all use this — single source of truth).

export type MeetingSummaryBlock =
  | { kind: "heading"; text: string; index?: string }
  | { kind: "paragraph"; text: string }
  | { kind: "bullet"; text: string; emoji?: string }
  | { kind: "quote"; text: string };

// Matches a single leading emoji (with optional variation selector) used as a
// bullet marker, e.g. "📍 Propose a price increase".
const LEADING_EMOJI = /^(\p{Extended_Pictographic}️?)\s+(.+)$/u;

const SUMMARY_HEADING_WORDS =
  /\b(meeting|mom|context|participant|stakeholder|client|agency|discussion|question|concern|clarification|explanation|option|expectation|success|decision|alignment|risk|caution|open point|action item|next step|follow-up|message|email|interpretation|background|current state|objective|requirement|proposal|deliverable|positioning|timeline|summary|notes|seo|geo|visibility|commercial|pricing)\b/i;

// Render inline markdown (**bold**, __bold__, *italic*) as React nodes so the
// generated MoM does not surface raw asterisks. Headings strip markup entirely
// since they are already styled bold.
export function renderInlineMarkdown(text: string): ReactNode[] {
  const nodes: ReactNode[] = [];
  const pattern = /(\*\*|__)(.+?)\1|(\*|_)(.+?)\3/g;
  let lastIndex = 0;
  let match: RegExpExecArray | null;
  let key = 0;
  while ((match = pattern.exec(text)) !== null) {
    if (match.index > lastIndex) {
      nodes.push(text.slice(lastIndex, match.index));
    }
    if (match[2] !== undefined) {
      nodes.push(
        <strong key={`b-${key++}`} className="font-bold text-foreground">
          {match[2]}
        </strong>,
      );
    } else if (match[4] !== undefined) {
      nodes.push(
        <em key={`i-${key++}`} className="italic">
          {match[4]}
        </em>,
      );
    }
    lastIndex = pattern.lastIndex;
  }
  if (lastIndex < text.length) nodes.push(text.slice(lastIndex));
  return nodes.length > 0 ? nodes : [text];
}

export function stripInlineMarkdown(text: string): string {
  return text.replace(/(\*\*|__|\*|_)(.+?)\1/g, "$2").replace(/[*_`]/g, "").trim();
}

function normalizeSummaryHeading(line: string): string | null {
  const isMarkdownHeading = /^#{1,6}\s+/.test(line);
  // A line that is entirely bold (e.g. "**Key Points**") is an emphasis-only
  // sub-header the model emits — treat it as a heading regardless of keyword.
  const isBoldOnly = /^(\*\*|__).+(\*\*|__)$/.test(line.trim());
  const stripped = line.replace(/^#{1,6}\s+/, "").replace(/\*\*/g, "").replace(/__/g, "").trim();
  const numbered = /^(\d{1,2})[.)]\s+(.+)$/.exec(stripped);
  const text = (numbered ? numbered[2] : stripped).replace(/:$/, "").trim();
  if (text.length < 3 || text.length > 120 || /[.!?]$/.test(text)) return null;
  if (
    !isMarkdownHeading
    && !isBoldOnly
    && !SUMMARY_HEADING_WORDS.test(text)
    && !/^[A-Z][A-Za-z0-9 /&-]{2,42}:$/.test(stripped)
  ) {
    return null;
  }
  return numbered ? `${numbered[1]}. ${text}` : text;
}

export function parseMeetingSummary(summary: string): MeetingSummaryBlock[] {
  const blocks: MeetingSummaryBlock[] = [];
  let paragraph: string[] = [];
  const flushParagraph = () => {
    const text = paragraph.join(" ").trim();
    if (text) blocks.push({ kind: "paragraph", text });
    paragraph = [];
  };

  for (const rawLine of summary.replace(/\r\n/g, "\n").split("\n")) {
    const line = rawLine.trim();
    if (!line) {
      flushParagraph();
      continue;
    }
    const heading = normalizeSummaryHeading(line);
    if (heading) {
      flushParagraph();
      const numbered = /^(\d{1,2})\.\s+(.+)$/.exec(heading);
      blocks.push(
        numbered
          ? { kind: "heading", text: numbered[2], index: numbered[1] }
          : { kind: "heading", text: heading },
      );
      continue;
    }
    const quote = /^>\s?(.+)$/.exec(line);
    if (quote) {
      flushParagraph();
      blocks.push({ kind: "quote", text: quote[1].trim() });
      continue;
    }
    const bullet = /^(?:[-*•]\s+|\d+[.)]\s+)(.+)$/.exec(line);
    if (bullet) {
      flushParagraph();
      blocks.push({ kind: "bullet", text: bullet[1].trim() });
      continue;
    }
    const emojiBullet = LEADING_EMOJI.exec(line);
    if (emojiBullet) {
      flushParagraph();
      blocks.push({ kind: "bullet", text: emojiBullet[2].trim(), emoji: emojiBullet[1] });
      continue;
    }
    paragraph.push(line);
  }
  flushParagraph();
  return blocks;
}

/** Render a meeting summary (or any compatible Markdown) as styled blocks —
 *  section headings, quotes, emoji bullets, paragraphs. Shared by the meeting
 *  Summary tab and the cross-meeting Digest report. */
export function MeetingSummaryContent({ summary }: { summary: string }) {
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

export function summaryLead(summary: string): string {
  const firstParagraph = parseMeetingSummary(summary).find((block) => block.kind === "paragraph");
  if (firstParagraph?.kind === "paragraph") return firstParagraph.text;
  return summary.trim().split(/\n+/)[0] ?? "";
}

function timestampToSeconds(hours: string | undefined, minutes: string, seconds: string): number {
  const h = hours ? Number.parseInt(hours, 10) : 0;
  return h * 3600 + Number.parseInt(minutes, 10) * 60 + Number.parseInt(seconds, 10);
}

// Render inline markdown AND linkify [mm:ss] / [h:mm:ss] timestamps into
// clickable buttons that seek the meeting audio.
export function renderRichInline(text: string, onSeek?: (seconds: number) => void): ReactNode[] {
  const nodes: ReactNode[] = [];
  const pattern = /(\*\*|__)(.+?)\1|(\*|_)(.+?)\3|\[(?:(\d{1,2}):)?(\d{1,2}):(\d{2})\]/g;
  let lastIndex = 0;
  let key = 0;
  let match: RegExpExecArray | null;
  while ((match = pattern.exec(text)) !== null) {
    if (match.index > lastIndex) nodes.push(text.slice(lastIndex, match.index));
    if (match[2] !== undefined) {
      nodes.push(
        <strong key={`b-${key++}`} className="font-bold text-foreground">{match[2]}</strong>,
      );
    } else if (match[4] !== undefined) {
      nodes.push(<em key={`i-${key++}`} className="italic">{match[4]}</em>);
    } else {
      const secs = timestampToSeconds(match[5], match[6], match[7]);
      const label = match[0];
      nodes.push(
        onSeek ? (
          <button
            key={`t-${key++}`}
            type="button"
            onClick={() => onSeek(secs)}
            className="font-semibold underline decoration-dotted underline-offset-2 transition-opacity hover:opacity-80"
            style={{ color: "hsl(var(--primary))" }}
            title="Jump to this point in the recording"
          >
            {label}
          </button>
        ) : (
          label
        ),
      );
    }
    lastIndex = pattern.lastIndex;
  }
  if (lastIndex < text.length) nodes.push(text.slice(lastIndex));
  return nodes.length > 0 ? nodes : [text];
}

// Markdown-formatted block renderer for chat answers: headings, bullets, and
// paragraphs with inline bold/italic and clickable timestamps.
export function MeetingRichText({ text, onSeek }: { text: string; onSeek?: (seconds: number) => void }) {
  return (
    <div className="space-y-2.5">
      {parseMeetingSummary(text).map((block, index) => {
        if (block.kind === "heading") {
          return (
            <h4 key={`${index}-${block.text}`} className="pt-1 text-[15px] font-bold text-foreground">
              {block.text}
            </h4>
          );
        }
        if (block.kind === "quote") {
          return (
            <p
              key={`${index}-${block.text}`}
              className="border-l-2 pl-3 text-[14px] italic leading-7 text-muted-foreground"
              style={{ borderColor: "hsl(var(--primary) / 0.6)" }}
            >
              {renderRichInline(block.text, onSeek)}
            </p>
          );
        }
        if (block.kind === "bullet") {
          return (
            <div key={`${index}-${block.text}`} className="flex gap-2.5 text-[14px] leading-7 text-foreground">
              {block.emoji ? (
                <span className="mt-0.5 flex-shrink-0 leading-7">{block.emoji}</span>
              ) : (
                <span className="mt-2.5 h-1 w-1 flex-shrink-0 rounded-full" style={{ background: "hsl(var(--primary))" }} />
              )}
              <p>{renderRichInline(block.text, onSeek)}</p>
            </div>
          );
        }
        return (
          <p key={`${index}-${block.text}`} className="text-[14px] leading-7 text-foreground">
            {renderRichInline(block.text, onSeek)}
          </p>
        );
      })}
    </div>
  );
}

export function ChatThinkingDots() {
  return (
    <div className="mt-2 flex items-center gap-2 text-[13px] text-muted-foreground">
      <span>AirNote is thinking</span>
      <span className="flex gap-1">
        {[0, 1, 2].map((i) => (
          <span
            key={i}
            className="h-1.5 w-1.5 animate-bounce rounded-full"
            style={{ background: "hsl(var(--primary))", animationDelay: `${i * 0.15}s` }}
          />
        ))}
      </span>
    </div>
  );
}
