import { useCallback } from "react";

interface Props {
  isRecording: boolean;
  duration: number;
  error: string | null;
  onStart: () => void;
  onStop: () => void;
  isSending: boolean;
}

export function RecordButton({ isRecording, duration, error, onStart, onStop, isSending }: Props) {
  const down = useCallback(() => { if (!isSending) onStart(); }, [onStart, isSending]);
  const up = useCallback(() => { if (isRecording) onStop(); }, [onStop, isRecording]);

  return (
    <div style={s.wrap}>
      {isRecording && (
        <>
          <div style={{ ...s.pulse, animationDelay: "0s" }} />
          <div style={{ ...s.pulse, animationDelay: "0.4s" }} />
        </>
      )}
      <button
        style={{ ...s.btn, ...(isRecording ? s.btnRec : {}), ...(isSending ? s.btnBusy : {}) }}
        onMouseDown={down} onMouseUp={up} onMouseLeave={up}
        onTouchStart={down} onTouchEnd={up}
        disabled={isSending}
        title={error || undefined}
      >
        {isSending ? (
          <div className="spinner" style={{ width: 18, height: 18 }} />
        ) : (
          <svg width="20" height="20" viewBox="0 0 24 24" fill={isRecording ? "var(--red)" : "none"} stroke={isRecording ? "var(--red)" : "var(--text-sec)"} strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <path d="M12 2a3 3 0 0 0-3 3v7a3 3 0 0 0 6 0V5a3 3 0 0 0-3-3Z" />
            <path d="M19 10v2a7 7 0 0 1-14 0v-2" />
            <line x1="12" x2="12" y1="19" y2="22" />
          </svg>
        )}
      </button>
      <span style={s.label}>
        {isSending ? "Processing..." : isRecording ? `${duration.toFixed(1)}s` : "Hold to record"}
      </span>
    </div>
  );
}

const s: Record<string, React.CSSProperties> = {
  wrap: {
    position: "relative",
    display: "flex", flexDirection: "column", alignItems: "center", gap: 4,
  },
  pulse: {
    position: "absolute", top: 0, left: 0, right: 0, bottom: 18,
    borderRadius: "50%",
    border: "2px solid var(--red)",
    opacity: 0,
    animation: "pulse-ring 1.5s ease-out infinite",
    pointerEvents: "none" as const,
  },
  btn: {
    width: 44, height: 44, borderRadius: "50%",
    border: "2px solid var(--border)",
    background: "var(--bg-elevated)",
    cursor: "pointer",
    display: "flex", alignItems: "center", justifyContent: "center",
    transition: "all 0.15s",
    outline: "none",
    position: "relative" as const, zIndex: 1,
  },
  btnRec: {
    borderColor: "var(--red)",
    background: "var(--red-bg)",
    boxShadow: "0 0 0 3px rgba(248,113,113,0.08)",
    transform: "scale(1.08)",
  },
  btnBusy: { opacity: 0.5, cursor: "not-allowed" },
  label: {
    fontSize: 9, fontWeight: 500, color: "var(--text-faint)",
    whiteSpace: "nowrap" as const, position: "absolute" as const, top: 50,
  },
};
