import type { PipelineTrace as T } from "../types";

export function PipelineTraceView({ trace }: { trace: T }) {
  return (
    <div className="fade-in">
      {/* Row 1: STT + Replacements */}
      <div style={s.row}>
        <Node num={1} title="Deepgram STT" timing={trace.stt_ms}>
          <Row label="Transcript"><Mono>{trace.deepgram_raw}</Mono></Row>
          <Row label="Confidence">
            <Badge c={trace.stt_confidence >= 0.9 ? "green" : trace.stt_confidence >= 0.7 ? "yellow" : "red"}>
              {(trace.stt_confidence * 100).toFixed(1)}%
            </Badge>
          </Row>
        </Node>
        <Node num={2} title="STT Replacements">
          {trace.stt_replacements_applied.length === 0 && trace.stt_near_misses.length === 0 ? (
            <Muted>No rules loaded</Muted>
          ) : (
            <>
              {trace.stt_replacements_applied.map((r, i) => (
                <div key={i} style={s.replItem}>
                  <Badge c={r.method === "exact" ? "green" : "yellow"}>{r.method}</Badge>
                  <span style={s.replFrom}>{r.from}</span>
                  <span style={{ color: "var(--text-faint)" }}>&rarr;</span>
                  <span style={s.replTo}>{r.to}</span>
                </div>
              ))}
              {trace.stt_near_misses.map((m, i) => (
                <div key={i} style={s.nearMiss}>
                  <span style={s.nearTokens}>"{m.best_token}" &asymp; "{m.rule_from}" &rarr; "{m.rule_to}"</span>
                  <Badge c={m.similarity >= 0.7 ? "yellow" : "red"}>{m.similarity.toFixed(2)}</Badge>
                </div>
              ))}
            </>
          )}
          <div style={s.sep}>
            <Row label="Result"><Mono>{trace.transcript_after_replacements}</Mono></Row>
          </div>
        </Node>
      </div>

      <Connector cols={2} />

      {/* Row 2: Vocab + Prompt */}
      <div style={s.row}>
        <Node num={3} title="Vocabulary">
          {trace.vocab_selected.length > 0 && (
            <>
              <div style={s.sectionLabel}>Selected ({trace.vocab_selected.length})</div>
              <div style={s.chipWrap}>
                {trace.vocab_selected.map((v, i) => (
                  <span key={i} style={s.vocabChip}>
                    {v.term}
                    {v.term_type && <Badge c="muted">{v.term_type}</Badge>}
                    <span style={{ fontSize: 10, color: "var(--text-faint)" }}>w={v.weight.toFixed(1)}</span>
                  </span>
                ))}
              </div>
            </>
          )}
          {trace.vocab_rejected_sample.length > 0 && (
            <details style={{ marginTop: 6 }}>
              <summary style={s.detailsSum}>Rejected ({trace.vocab_rejected_sample.length})</summary>
              <div style={s.detailsBody}>
                {trace.vocab_rejected_sample.map((v, i) => (
                  <div key={i} style={{ fontSize: 11, color: "var(--text-faint)", padding: "2px 0", fontFamily: "var(--mono)" }}>
                    {v.term} &middot; {v.reason} ({v.best_phonetic_sim.toFixed(2)})
                  </div>
                ))}
              </div>
            </details>
          )}
          {trace.rag_examples_used > 0 && (
            <Row label="RAG"><Badge c="muted">{trace.rag_examples_used} examples</Badge></Row>
          )}
          {trace.vocab_selected.length === 0 && trace.vocab_rejected_sample.length === 0 && (
            <Muted>No vocabulary terms</Muted>
          )}
        </Node>

        <Node num={4} title="System Prompt">
          <Row label="Length"><Mono>{trace.system_prompt_length.toLocaleString()} chars</Mono></Row>
          {trace.system_prompt_preview && (
            <details style={{ marginTop: 6 }}>
              <summary style={s.detailsSum}>View prompt</summary>
              <pre style={s.promptPre}>{trace.system_prompt_preview}</pre>
            </details>
          )}
        </Node>
      </div>

      <Connector cols={2} />

      {/* Row 3: LLM + Post */}
      <div style={s.row}>
        <Node num={5} title="LLM Polish" timing={trace.llm_ms}>
          <div style={s.polishBox}>{trace.polished}</div>
          <Row label="Model"><Badge c="muted">{trace.llm_model}</Badge></Row>
        </Node>

        <Node num={6} title="Post-processing">
          <div style={s.postGrid}>
            <PostItem label="Devanagari" value={trace.devanagari_detected ? "recovered" : "clean"} warn={trace.devanagari_detected} />
            <PostItem label="Format" value={trace.format_recovered ? "fixed" : "clean"} warn={trace.format_recovered} />
            <PostItem label="Guard" value={trace.content_guard_triggered ? "TRIGGERED" : "clean"} warn={trace.content_guard_triggered} />
          </div>
        </Node>
      </div>

      <Connector cols={1} />

      {/* Final */}
      <div style={s.finalCard}>
        <div style={s.finalHead}>
          <span>Final Output</span>
          <span style={{ fontFamily: "var(--mono)" }}>{trace.total_ms}ms total</span>
        </div>
        <div style={s.finalBody}>{trace.final_output}</div>
      </div>
    </div>
  );
}

/* ── Sub-components ──────────────────────────────────── */

function Node({ num, title, timing, children }: { num: number; title: string; timing?: number; children: React.ReactNode }) {
  return (
    <div style={s.node}>
      <div style={s.nodeHead}>
        <div style={s.nodeHeadLeft}>
          <span style={s.nodeNum}>{num}</span>
          <span style={s.nodeTitle}>{title}</span>
        </div>
        {timing !== undefined && <span style={s.nodeTiming}>{timing}ms</span>}
      </div>
      <div style={s.nodeBody}>{children}</div>
    </div>
  );
}

function Connector({ cols }: { cols: number }) {
  if (cols === 1) {
    return (
      <div style={s.connectorSingle}>
        <div style={s.connectorLine} />
      </div>
    );
  }
  return (
    <div style={s.connectorRow}>
      <div style={s.connectorCell}><div style={s.connectorLine} /></div>
      <div style={s.connectorCell}><div style={s.connectorLine} /></div>
    </div>
  );
}

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return <div style={s.rowFlex}><span style={s.rowLabel}>{label}</span>{children}</div>;
}
function Mono({ children }: { children: React.ReactNode }) {
  return <span style={s.mono}>{children}</span>;
}
function Muted({ children }: { children: React.ReactNode }) {
  return <span style={{ fontSize: 11, color: "var(--text-faint)", fontStyle: "italic" }}>{children}</span>;
}

function Badge({ c, children }: { c: "green" | "yellow" | "red" | "muted"; children: React.ReactNode }) {
  const m: Record<string, { bg: string; bc: string; fg: string }> = {
    green: { bg: "var(--green-bg)", bc: "var(--green-border)", fg: "var(--green)" },
    yellow: { bg: "var(--yellow-bg)", bc: "var(--yellow-border)", fg: "var(--yellow)" },
    red: { bg: "var(--red-bg)", bc: "var(--red-border)", fg: "var(--red)" },
    muted: { bg: "rgba(161,161,170,0.05)", bc: "var(--border)", fg: "var(--text-sec)" },
  };
  const v = m[c];
  return (
    <span style={{ fontSize: 10, fontWeight: 600, fontFamily: "var(--mono)", padding: "2px 7px", borderRadius: 4, background: v.bg, border: `1px solid ${v.bc}`, color: v.fg, whiteSpace: "nowrap" }}>
      {children}
    </span>
  );
}

function PostItem({ label, value, warn }: { label: string; value: string; warn?: boolean }) {
  return (
    <div style={s.postItem}>
      <div style={s.postLabel}>{label}</div>
      <div style={{ fontSize: 12, fontFamily: "var(--mono)", fontWeight: 500, color: warn ? "var(--yellow)" : "var(--text-muted)", marginTop: 1 }}>{value}</div>
    </div>
  );
}

/* ── Styles ──────────────────────────────────────────── */

const s: Record<string, React.CSSProperties> = {
  row: { display: "grid", gridTemplateColumns: "1fr 1fr", gap: 28 },

  // Node card
  node: { background: "var(--bg-card)", border: "1px solid var(--border)", borderRadius: "var(--radius)", overflow: "hidden" },
  nodeHead: { display: "flex", justifyContent: "space-between", alignItems: "center", padding: "9px 14px", borderBottom: "1px solid var(--border-subtle)", background: "var(--bg-elevated)" },
  nodeHeadLeft: { display: "flex", alignItems: "center", gap: 8 },
  nodeNum: { width: 20, height: 20, borderRadius: 5, background: "var(--accent-bg)", color: "var(--accent)", fontSize: 11, fontWeight: 700, fontFamily: "var(--mono)", display: "flex", alignItems: "center", justifyContent: "center", border: "1px solid rgba(129,140,248,0.15)" },
  nodeTitle: { fontSize: 12, fontWeight: 600, color: "var(--text-sec)" },
  nodeTiming: { fontSize: 11, fontFamily: "var(--mono)", color: "var(--text-muted)", padding: "2px 7px", borderRadius: 4, background: "rgba(161,161,170,0.05)", border: "1px solid var(--border)" },
  nodeBody: { padding: "12px 14px" },

  // Connector
  connectorRow: { display: "grid", gridTemplateColumns: "1fr 1fr", gap: "0 28px", height: 32 },
  connectorSingle: { display: "flex", justifyContent: "center", height: 32 },
  connectorCell: { display: "flex", justifyContent: "center" },
  connectorLine: { width: 2, height: "100%", background: "var(--node-line)", position: "relative" as const },

  // Row helpers
  rowFlex: { display: "flex", alignItems: "center", gap: 8, padding: "3px 0", flexWrap: "wrap" as const },
  rowLabel: { fontSize: 11, fontWeight: 500, color: "var(--text-muted)", minWidth: 70 },
  mono: { fontFamily: "var(--mono)", fontSize: 13, color: "var(--text)", lineHeight: 1.6 },

  // Replacements
  replItem: { display: "flex", alignItems: "center", gap: 8, padding: "5px 10px", background: "var(--bg-inset)", borderRadius: 6, marginBottom: 4, border: "1px solid var(--border-subtle)" },
  replFrom: { color: "var(--text-muted)", textDecoration: "line-through", fontFamily: "var(--mono)", fontSize: 12 },
  replTo: { color: "var(--green)", fontFamily: "var(--mono)", fontSize: 12, fontWeight: 500 },
  nearMiss: { display: "flex", alignItems: "center", justifyContent: "space-between", padding: "4px 10px", background: "var(--bg-inset)", borderRadius: 5, marginTop: 4, border: "1px solid var(--border-subtle)" },
  nearTokens: { fontFamily: "var(--mono)", fontSize: 11, color: "var(--text-sec)" },
  sep: { marginTop: 8, paddingTop: 8, borderTop: "1px solid var(--border-subtle)" },

  // Vocab
  sectionLabel: { marginBottom: 6, fontSize: 10, fontWeight: 600, color: "var(--text-faint)", textTransform: "uppercase" as const, letterSpacing: "0.06em" },
  chipWrap: { display: "flex", flexWrap: "wrap" as const, gap: 4 },
  vocabChip: { display: "inline-flex", alignItems: "center", gap: 5, padding: "4px 10px", background: "var(--bg-inset)", border: "1px solid var(--green-border)", borderRadius: 6, fontFamily: "var(--mono)", fontSize: 12, color: "var(--green)", fontWeight: 600 },
  detailsSum: { fontSize: 11, fontWeight: 500, color: "var(--text-muted)", padding: "3px 0" },
  detailsBody: { marginTop: 4, padding: "6px 10px", background: "var(--bg-inset)", borderRadius: 6, border: "1px solid var(--border-subtle)" },

  // Prompt
  promptPre: { fontFamily: "var(--mono)", fontSize: 11, color: "var(--text-muted)", whiteSpace: "pre-wrap" as const, wordBreak: "break-word" as const, background: "var(--bg-inset)", padding: "10px 12px", borderRadius: 6, border: "1px solid var(--border-subtle)", marginTop: 6, maxHeight: 180, overflow: "auto", lineHeight: 1.6 },

  // LLM
  polishBox: { fontFamily: "var(--mono)", fontSize: 13, padding: "8px 12px", background: "var(--bg-inset)", borderRadius: 6, border: "1px solid var(--border-subtle)", lineHeight: 1.6, marginBottom: 8 },

  // Post-processing
  postGrid: { display: "grid", gridTemplateColumns: "repeat(3, 1fr)", gap: 6 },
  postItem: { padding: "7px 10px", background: "var(--bg-inset)", border: "1px solid var(--border-subtle)", borderRadius: 6 },
  postLabel: { fontSize: 10, color: "var(--text-faint)", fontWeight: 500, textTransform: "uppercase" as const, letterSpacing: "0.04em" },

  // Final
  finalCard: { background: "var(--bg-card)", border: "1.5px solid var(--green-border)", borderRadius: "var(--radius)", overflow: "hidden", boxShadow: "0 0 0 1px var(--green-border), 0 4px 12px rgba(74,222,128,0.04)" },
  finalHead: { display: "flex", justifyContent: "space-between", alignItems: "center", padding: "9px 14px", background: "var(--green-bg)", borderBottom: "1px solid var(--green-border)", fontSize: 12, fontWeight: 600, color: "var(--green)" },
  finalBody: { padding: "14px 16px", fontFamily: "var(--mono)", fontSize: 14, lineHeight: 1.7, fontWeight: 500 },
};
