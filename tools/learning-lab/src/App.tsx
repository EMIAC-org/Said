import { useState, useCallback, useRef, useEffect } from "react";
import { useAudioRecorder } from "./hooks/useAudioRecorder";
import { Sidebar } from "./components/Sidebar";
import { RecordButton } from "./components/RecordButton";
import { PipelineTraceView } from "./components/PipelineTrace";
import { HistoryPanel } from "./components/HistoryPanel";
import type { PipelineTrace, HistoryEntry } from "./types";

const API_URL = "http://127.0.0.1:48484/v1/lab/trace";

function App() {
  const recorder = useAudioRecorder();
  const [trace, setTrace] = useState<PipelineTrace | null>(null);
  const [history, setHistory] = useState<HistoryEntry[]>([]);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [isSending, setIsSending] = useState(false);
  const [backendOk, setBackendOk] = useState(false);
  const [toasts, setToasts] = useState<{ id: number; msg: string }[]>([]);
  const toastIdRef = useRef(0);
  const idCounter = useRef(0);

  const showToast = useCallback((msg: string) => {
    const id = ++toastIdRef.current;
    setToasts((prev) => [...prev, { id, msg }]);
    setTimeout(() => setToasts((prev) => prev.filter((t) => t.id !== id)), 4000);
  }, []);

  const sendAudio = useCallback(async (blob: Blob) => {
    setIsSending(true);
    setError(null);
    try {
      const form = new FormData();
      form.append("audio", blob, "recording.wav");
      const res = await fetch(API_URL, { method: "POST", body: form });
      if (!res.ok) throw new Error(`${res.status}: ${await res.text()}`);
      const data: PipelineTrace = await res.json();
      setTrace(data);
      setBackendOk(true);
      const id = ++idCounter.current;
      setHistory((prev) => [
        { id, timestamp: new Date(), trace: data, audioUrl: URL.createObjectURL(blob) },
        ...prev,
      ]);
      setSelectedId(id);
    } catch (err) {
      const raw = err instanceof TypeError
        ? "Cannot reach backend"
        : err instanceof Error ? err.message : "Unknown error";
      // Parse JSON error body if present
      const jsonMatch = raw.match(/\{"error":"(.+?)"\}/);
      const cleaned = jsonMatch ? jsonMatch[1] : raw;
      // Strip HTTP status prefix like "502: "
      const msg = cleaned.replace(/^\d{3}:\s*/, "");
      setBackendOk(err instanceof TypeError ? false : true);
      showToast(msg);
    } finally {
      setIsSending(false);
    }
  }, []);

  const handleStop = useCallback(() => recorder.stopRecording(), [recorder]);

  const prevBlobRef = useRef<Blob | null>(null);
  if (recorder.audioBlob && recorder.audioBlob !== prevBlobRef.current && !recorder.isRecording) {
    prevBlobRef.current = recorder.audioBlob;
    sendAudio(recorder.audioBlob);
  }

  const handleHistorySelect = useCallback((entry: HistoryEntry) => {
    setSelectedId(entry.id);
    setTrace(entry.trace);
  }, []);

  const stats = {
    runs: history.length,
    avgStt: history.length ? Math.round(history.reduce((a, e) => a + e.trace.stt_ms, 0) / history.length) : 0,
    avgTotal: history.length ? Math.round(history.reduce((a, e) => a + e.trace.total_ms, 0) / history.length) : 0,
    fixes: history.reduce((a, e) => a + e.trace.stt_replacements_applied.length, 0),
    vocabHit: history.length
      ? Math.round(
          (history.filter((e) => e.trace.vocab_selected.length > 0).length / history.length) * 100
        )
      : 0,
  };

  return (
    <div style={s.shell}>
      <Sidebar backendOk={backendOk} />

      {/* ── Top bar ────────────────────────────────────── */}
      <div style={s.topbar}>
        <div style={s.statsRow}>
          <Stat label="Runs" value={stats.runs.toString()} />
          <Stat label="Avg STT" value={`${stats.avgStt}ms`} color={stats.avgStt > 0 && stats.avgStt < 800 ? "var(--green)" : undefined} />
          <Stat label="Avg Total" value={stats.avgTotal > 0 ? `${(stats.avgTotal / 1000).toFixed(1)}s` : "—"} color={stats.avgTotal > 0 && stats.avgTotal < 1500 ? "var(--green)" : undefined} />
          <Stat label="STT Fixes" value={stats.fixes.toString()} color="var(--accent)" />
          <Stat label="Vocab Hit" value={stats.vocabHit > 0 ? `${stats.vocabHit}%` : "—"} color={stats.vocabHit > 50 ? "var(--green)" : stats.vocabHit > 0 ? "var(--yellow)" : undefined} />
        </div>
        <RecordButton
          isRecording={recorder.isRecording}
          duration={recorder.duration}
          error={recorder.error}
          onStart={recorder.startRecording}
          onStop={handleStop}
          isSending={isSending}
        />
      </div>

      {/* ── Toast container ────────────────────────────── */}
      <div style={s.toastContainer}>
        {toasts.map((t) => (
          <Toast key={t.id} msg={t.msg} onDismiss={() => setToasts((prev) => prev.filter((x) => x.id !== t.id))} />
        ))}
      </div>

      {/* ── Main content ───────────────────────────────── */}
      <main style={s.main}>
        {isSending && (
          <div style={s.loading} className="fade-in">
            <div className="spinner" />
            Running pipeline...
          </div>
        )}

        {trace && !isSending && <PipelineTraceView trace={trace} />}

        {!trace && !isSending && (
          <div style={s.placeholder}>
            <svg width="36" height="36" viewBox="0 0 24 24" fill="none" stroke="var(--text-faint)" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
              <path d="M12 2a3 3 0 0 0-3 3v7a3 3 0 0 0 6 0V5a3 3 0 0 0-3-3Z" />
              <path d="M19 10v2a7 7 0 0 1-14 0v-2" /><line x1="12" x2="12" y1="19" y2="22" />
            </svg>
            <p>Hold the button and speak to trace the pipeline</p>
          </div>
        )}
      </main>

      {/* ── History sidebar ────────────────────────────── */}
      <aside style={s.historySidebar}>
        <HistoryPanel entries={history} selectedId={selectedId} onSelect={handleHistorySelect} />
      </aside>
    </div>
  );
}

function Toast({ msg, onDismiss }: { msg: string; onDismiss: () => void }) {
  const [exiting, setExiting] = useState(false);
  useEffect(() => {
    const t = setTimeout(() => setExiting(true), 3500);
    return () => clearTimeout(t);
  }, []);
  return (
    <div
      style={{
        ...s.toast,
        animation: exiting ? "toast-out 0.3s ease-in forwards" : "toast-in 0.3s ease-out",
      }}
      onClick={onDismiss}
    >
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="var(--red)" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" style={{ flexShrink: 0 }}>
        <circle cx="12" cy="12" r="10" /><line x1="12" x2="12" y1="8" y2="12" /><line x1="12" x2="12.01" y1="16" y2="16" />
      </svg>
      <span style={s.toastMsg}>{msg}</span>
    </div>
  );
}

function Stat({ label, value, color }: { label: string; value: string; color?: string }) {
  return (
    <div style={s.stat}>
      <div style={s.statLabel}>{label}</div>
      <div style={{ ...s.statValue, color: color || "var(--text)" }}>{value}</div>
    </div>
  );
}

const s: Record<string, React.CSSProperties> = {
  shell: {
    display: "grid",
    gridTemplateColumns: "210px 1fr 330px",
    gridTemplateRows: "auto 1fr",
    height: "100vh",
  },

  // Top bar
  topbar: {
    gridColumn: "2 / -1",
    padding: "14px 24px",
    borderBottom: "1px solid var(--border-subtle)",
    background: "var(--bg-card)",
    display: "flex",
    alignItems: "center",
    gap: 16,
  },
  statsRow: { display: "flex", gap: 10, flex: 1 },
  stat: {
    padding: "7px 14px",
    background: "var(--bg-elevated)",
    border: "1px solid var(--border)",
    borderRadius: "var(--radius-sm)",
    minWidth: 100,
  },
  statLabel: {
    fontSize: 10, fontWeight: 600,
    color: "var(--text-faint)",
    textTransform: "uppercase" as const,
    letterSpacing: "0.05em",
  },
  statValue: {
    fontSize: 18, fontWeight: 700,
    fontFamily: "var(--mono)",
    letterSpacing: "-0.02em",
    marginTop: 1,
  },

  // Main
  main: {
    gridColumn: "2",
    overflowY: "auto",
    padding: "24px 28px 60px",
  },

  // History
  historySidebar: {
    gridColumn: "3",
    gridRow: "2",
    borderLeft: "1px solid var(--border-subtle)",
    background: "var(--bg-card)",
    overflowY: "auto",
  },

  // Toast
  toastContainer: {
    position: "fixed" as const,
    bottom: 24,
    left: "50%",
    transform: "translateX(-50%)",
    zIndex: 999,
    display: "flex",
    flexDirection: "column" as const,
    gap: 8,
    pointerEvents: "none" as const,
  },
  toast: {
    display: "flex", alignItems: "center", gap: 10,
    background: "#1c1012",
    border: "1px solid var(--red-border)",
    borderRadius: 10,
    padding: "10px 16px",
    color: "var(--red)",
    fontSize: 13,
    fontFamily: "var(--mono)",
    boxShadow: "0 8px 24px rgba(0,0,0,0.5), 0 0 0 1px var(--red-border)",
    pointerEvents: "auto" as const,
    cursor: "pointer",
    maxWidth: 480,
    backdropFilter: "blur(12px)",
  },
  toastMsg: { lineHeight: 1.4 },

  // Feedback
  loading: {
    display: "flex", alignItems: "center", gap: 10,
    color: "var(--text-muted)", fontSize: 13, fontWeight: 500,
    padding: 40, justifyContent: "center",
  },
  placeholder: {
    display: "flex", flexDirection: "column", alignItems: "center",
    gap: 12, color: "var(--text-faint)", fontSize: 13,
    padding: 60, textAlign: "center" as const,
  },
};

export default App;
