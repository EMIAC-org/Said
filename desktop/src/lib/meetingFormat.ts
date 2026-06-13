// Shared meeting formatting helpers — one source of truth for speaker colors and
// timestamp formatting across the live view and the post-meeting view. Previously
// each view had its own (drifted) copies: two different speaker palettes (so the
// same speaker showed different colors live vs. post-meeting) and a live
// formatter that couldn't render hours (a 73-minute meeting showed "73:20").

export const SPEAKER_COLORS = [
  "hsl(226 80% 78%)", // periwinkle (primary)
  "hsl(142 70% 65%)", // green
  "hsl(38 90% 72%)", // amber
  "hsl(354 85% 75%)", // red
  "hsl(280 70% 75%)", // purple
  "hsl(180 60% 65%)", // teal
];

/**
 * Stable, stateless color for a speaker id — the SAME id maps to the SAME color
 * everywhere (live and post-meeting), with no shared mutable map. "You"/the mic
 * gets the primary color; `speaker_N` ids fan out across the palette; anything
 * else hashes deterministically.
 */
export function speakerColor(speakerId: string): string {
  const id = speakerId.trim().toLowerCase();
  if (id === "you" || id === "mic") return SPEAKER_COLORS[0];
  const numbered = /(\d+)\s*$/.exec(id);
  if (numbered) return SPEAKER_COLORS[Number(numbered[1]) % SPEAKER_COLORS.length];
  let hash = 0;
  for (let i = 0; i < id.length; i++) hash = (hash * 31 + id.charCodeAt(i)) >>> 0;
  return SPEAKER_COLORS[hash % SPEAKER_COLORS.length];
}

/** Format a millisecond offset as `m:ss`, or `h:mm:ss` once it passes an hour. */
export function formatTimestamp(ms: number): string {
  const totalSeconds = Math.max(0, Math.floor(ms / 1000));
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;
  if (hours > 0) {
    return `${hours}:${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}`;
  }
  return `${minutes}:${String(seconds).padStart(2, "0")}`;
}
