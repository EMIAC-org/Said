import type { HistoryEntry } from "../types";

interface Props {
  entries: HistoryEntry[];
  selectedId: number | null;
  onSelect: (entry: HistoryEntry) => void;
}

export function HistoryPanel({ entries, selectedId, onSelect }: Props) {
  if (entries.length === 0) {
    return (
      <div style={s.empty}>
        <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="var(--text-faint)" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
          <path d="M12 2a3 3 0 0 0-3 3v7a3 3 0 0 0 6 0V5a3 3 0 0 0-3-3Z" />
          <path d="M19 10v2a7 7 0 0 1-14 0v-2" /><line x1="12" x2="12" y1="19" y2="22" />
        </svg>
        <div style={s.emptyText}>Recordings will appear here.<br />Hold the mic and speak.</div>
      </div>
    );
  }

  return (
    <div style={s.container}>
      <div style={s.head}>
        <h3 style={s.headTitle}>History</h3>
        <span style={s.headCount}>{entries.length}</span>
      </div>
      <div style={s.list}>
        {entries.map((entry) => (
          <button
            key={entry.id}
            style={{ ...s.entry, ...(selectedId === entry.id ? s.entryActive : {}) }}
            onClick={() => onSelect(entry)}
          >
            <div style={s.entryTop}>
              <span style={s.time}>
                {entry.timestamp.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" })}
              </span>
              <span style={{
                ...s.badge,
                ...(entry.trace.total_ms < 1200 ? s.badgeGreen : entry.trace.total_ms < 1800 ? s.badgeYellow : s.badgeRed),
              }}>
                {entry.trace.total_ms}ms
              </span>
            </div>
            <div style={s.raw}>{trunc(entry.trace.local_speech_raw, 50)}</div>
            <div style={s.output}>{trunc(entry.trace.final_output, 50)}</div>
            {entry.trace.stt_replacements_applied.length > 0 && (
              <div style={s.tags}>
                {entry.trace.stt_replacements_applied.map((r, i) => (
                  <span key={i} style={s.tag}>{r.from} &rarr; {r.to}</span>
                ))}
              </div>
            )}
            {entry.trace.stt_near_misses.length > 0 && entry.trace.stt_replacements_applied.length === 0 && (
              <div style={s.miss}>
                &#9888; near miss: {entry.trace.stt_near_misses[0].best_token} (sim={entry.trace.stt_near_misses[0].similarity.toFixed(2)})
              </div>
            )}
          </button>
        ))}
      </div>
    </div>
  );
}

function trunc(s: string, n: number) { return s.length > n ? s.slice(0, n) + "…" : s; }

const s: Record<string, React.CSSProperties> = {
  container: { display: "flex", flexDirection: "column", height: "100%" },
  head: { padding: "14px 16px", borderBottom: "1px solid var(--border-subtle)", display: "flex", justifyContent: "space-between", alignItems: "center" },
  headTitle: { fontSize: 13, fontWeight: 600, color: "var(--text-sec)", margin: 0 },
  headCount: { fontSize: 10, fontFamily: "var(--mono)", fontWeight: 600, color: "var(--text-muted)", background: "var(--bg-elevated)", border: "1px solid var(--border)", borderRadius: 4, padding: "1px 6px" },
  list: { flex: 1, overflowY: "auto" as const, padding: 6, display: "flex", flexDirection: "column", gap: 3 },
  entry: { display: "block", width: "100%", textAlign: "left" as const, background: "transparent", border: "1px solid transparent", borderRadius: "var(--radius-sm)", padding: "10px 12px", cursor: "pointer", color: "var(--text)", transition: "all 0.1s", fontFamily: "inherit" },
  entryActive: { background: "var(--accent-bg)", borderColor: "rgba(129,140,248,0.18)" },
  entryTop: { display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 4 },
  time: { fontSize: 10, fontFamily: "var(--mono)", color: "var(--text-faint)", fontWeight: 500 },
  badge: { fontSize: 10, fontFamily: "var(--mono)", fontWeight: 600, padding: "1px 6px", borderRadius: 4, border: "1px solid" },
  badgeGreen: { background: "var(--green-bg)", borderColor: "var(--green-border)", color: "var(--green)" },
  badgeYellow: { background: "var(--yellow-bg)", borderColor: "var(--yellow-border)", color: "var(--yellow)" },
  badgeRed: { background: "var(--red-bg)", borderColor: "var(--red-border)", color: "var(--red)" },
  raw: { fontSize: 11, fontFamily: "var(--mono)", color: "var(--text-muted)", overflow: "hidden", textOverflow: "ellipsis" as const, whiteSpace: "nowrap" as const, marginBottom: 2 },
  output: { fontSize: 12, fontFamily: "var(--mono)", color: "var(--text)", fontWeight: 500, overflow: "hidden", textOverflow: "ellipsis" as const, whiteSpace: "nowrap" as const },
  tags: { display: "flex", gap: 4, marginTop: 5, flexWrap: "wrap" as const },
  tag: { fontSize: 9, fontFamily: "var(--mono)", color: "var(--green)", background: "var(--green-bg)", border: "1px solid var(--green-border)", borderRadius: 3, padding: "0 5px" },
  miss: { fontSize: 9, fontFamily: "var(--mono)", color: "var(--red)", marginTop: 3 },
  empty: { display: "flex", flexDirection: "column", alignItems: "center", justifyContent: "center", height: "100%", padding: 40, gap: 12 },
  emptyText: { fontSize: 12, color: "var(--text-faint)", textAlign: "center" as const, lineHeight: 1.7 },
};
