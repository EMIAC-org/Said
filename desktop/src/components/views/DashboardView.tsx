import { useEffect, useState } from "react";
import { Mic } from "lucide-react";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  DonutCard,
  TimeSavedCard,
  PaceCard,
  RecordingsTable,
  TimelineCard,
  AppBreakdownCard,
} from "@/components/DashboardCards";
import { listHistory } from "@/lib/invoke";
import type { AppSnapshot, PendingEdit, Recording } from "@/types";

// ── Props ──────────────────────────────────────────────────────────────────────

interface DashboardViewProps {
  snapshot:        AppSnapshot | null;
  busy:            boolean;
  onToggle:        () => void;
  onAccessibility: () => void;
  onNavigate?:     (view: string) => void;
  statusPhase?:    string;
  liveText?:       string;
  pendingEdits?:   PendingEdit[];
  onResolvePending?: (id: string, action: "approve" | "skip") => void;
  onDownloadSuccess?: (path: string) => void;
}

// ── View ───────────────────────────────────────────────────────────────────────

export function DashboardView({
  snapshot,
  busy,
  onToggle: _onToggle,
  onAccessibility,
  onNavigate,
  statusPhase    = "",
  liveText       = "",
  onDownloadSuccess,
}: DashboardViewProps) {
  const [recordings, setRecordings] = useState<Recording[]>([]);

  const isRecording  = snapshot?.state === "recording";
  const isProcessing = snapshot?.state === "processing" || busy;

  const history = snapshot?.history ?? [];

  // Fetch full Recording[] — 300 records gives ~30 days of data for the
  // timeline and app breakdown cards without a separate API call.
  useEffect(() => {
    let alive = true;
    listHistory(300).then((r) => { if (alive) setRecordings(r); });
    return () => { alive = false; };
  }, [history.length]);

  return (
    <ScrollArea className="h-full">
      <div className="px-7 pt-4 pb-10 max-w-[1280px] mx-auto">

        {/* ── Live streaming preview ──────────────────── */}
        {(statusPhase || liveText) && (
          <div className="rounded-2xl panel px-5 py-4 mb-5 relative overflow-hidden">
            <div className="relative flex items-center gap-2 mb-2">
              <span
                className="w-1.5 h-1.5 rounded-full flex-shrink-0 animate-pulse"
                style={{
                  background: statusPhase === "transcribing"
                    ? "hsl(38 90% 55%)"
                    : "hsl(var(--primary))",
                  boxShadow: "0 0 8px currentColor",
                }}
              />
              <span
                className="text-[10px] font-bold uppercase tracking-[0.14em]"
                style={{ color: "hsl(var(--chip-mint-fg))" }}
              >
                {statusPhase === "transcribing" ? "Transcribing audio…" : "Polishing with LLM…"}
              </span>
            </div>
            {liveText && (
              <p className="relative text-[14px] text-foreground leading-relaxed">
                {liveText}
                <span className="caret-blink" />
              </p>
            )}
          </div>
        )}

        {/* ─────────────────────────────────────────────────────────────────
           Single-column flow — full-width main content
           ─────────────────────────────────────────────────────────────── */}
        <div className="space-y-4">

          {/* ── 3 stat tiles: WPM · Total Words · Time Saved ── */}
          <div
            className="grid gap-4"
            style={{
              gridTemplateColumns: "repeat(auto-fit, minmax(200px, 1fr))",
            }}
          >
            <PaceCard snapshot={snapshot} />
            <DonutCard
              snapshot={snapshot}
              isRecording={isRecording}
              isProcessing={isProcessing}
            />
            <TimeSavedCard snapshot={snapshot} />
          </div>

          {/* ── Words over time — 7d / 30d bar chart ── */}
          <TimelineCard recordings={recordings} />

          {/* ── Fixes by app ── */}
          <AppBreakdownCard recordings={recordings} />

          {/* ── Recent recordings ── */}
          <RecordingsTable
            recordings={recordings.slice(0, 10)}
            onSeeAll={() => onNavigate?.("history")}
            onDownloadSuccess={onDownloadSuccess}
          />
        </div>

        {/* ── Accessibility nudge ────────────────────── */}
        {snapshot && !snapshot.accessibility_granted && snapshot.auto_paste_supported && (
          <div
            className="mt-5 rounded-2xl px-5 py-4 flex items-start justify-between gap-4"
            style={{
              background: "hsl(var(--chip-amber-bg))",
              boxShadow:  "inset 0 0 0 1px hsl(var(--chip-amber-fg) / 0.20)",
            }}
          >
            <div className="flex items-start gap-3">
              <span
                className="flex items-center justify-center w-8 h-8 rounded-lg flex-shrink-0 mt-0.5"
                style={{
                  background: "hsl(var(--chip-amber-fg) / 0.20)",
                  color:      "hsl(var(--chip-amber-fg))",
                }}
              >
                <Mic size={14} />
              </span>
              <div>
                <p className="text-[13px] font-semibold" style={{ color: "hsl(var(--chip-amber-fg))" }}>
                  Enable auto-paste
                </p>
                <p
                  className="text-[11.5px] mt-1 leading-relaxed"
                  style={{ color: "hsl(var(--chip-amber-fg) / 0.85)" }}
                >
                  Grant Accessibility access so Said can paste polished text directly into any app.
                </p>
              </div>
            </div>
            <button onClick={onAccessibility} className="btn-primary flex-shrink-0">
              Enable
            </button>
          </div>
        )}

      </div>
    </ScrollArea>
  );
}
