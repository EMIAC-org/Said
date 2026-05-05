import { useMemo } from "react";
import { Keyboard } from "lucide-react";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Progress } from "@/components/ui/progress";
import { ScrollArea } from "@/components/ui/scroll-area";
import { groupHistory } from "@/types";
import type { AppSnapshot } from "@/types";

interface HomeViewProps {
  snapshot: AppSnapshot | null;
  busy: boolean;
  onToggle: () => void;
  onMode: (key: string) => void;
  onAccessibility: () => void;
}

function statusLabel(snapshot: AppSnapshot | null, busy: boolean): string {
  if (!snapshot) return "Preparing";
  if (busy || snapshot.state === "processing") return "Processing…";
  if (snapshot.state === "recording") return "Recording";
  return "Ready";
}

/** Progress toward unlocking the voice profile (unlock at 1,400 words) */
function profileProgress(total: number): { pct: number; wordsLeft: string } {
  const target = 1_400;
  const pct = Math.min((total / target) * 100, 100);
  const left = Math.max(target - total, 0);
  return { pct, wordsLeft: `${left.toLocaleString()} words` };
}

export function HomeView({
  snapshot,
  busy,
  onToggle,
  onMode,
  onAccessibility,
}: HomeViewProps) {
  const isRecording = snapshot?.state === "recording";
  const isProcessing = snapshot?.state === "processing" || busy;
  const result = snapshot?.last_result;

  const timeline = useMemo(
    () => groupHistory(snapshot?.history ?? []),
    [snapshot?.history]
  );

  const { pct: profilePct, wordsLeft } = profileProgress(
    snapshot?.total_words ?? 0
  );

  return (
    <ScrollArea className="h-full">
      <div className="p-5 pb-8">
        {/* 2-column grid */}
        <div className="grid gap-5" style={{ gridTemplateColumns: "minmax(0,1fr) 280px" }}>

          {/* ── LEFT MAIN ── */}
          <div className="min-w-0">

            {/* Header */}
            <header className="flex items-start justify-between gap-4 mb-5">
              <h1 className="text-xl font-semibold leading-snug text-foreground">
                Hey, get back into the flow with{" "}
                <span className="inline-flex items-center gap-1 align-middle">
                  <kbd className="inline-flex items-center gap-1 rounded border border-border bg-secondary px-2 py-0.5 text-sm font-semibold text-foreground shadow-sm">
                    <Keyboard size={13} className="text-muted-foreground" />
                    Caps Lock
                  </kbd>
                </span>
              </h1>

              {/* Mode pills */}
              <div className="flex flex-wrap gap-1.5 flex-shrink-0">
                {(snapshot?.modes ?? []).map((mode) => (
                  <button
                    key={mode.key}
                    onClick={() => onMode(mode.key)}
                    disabled={isProcessing}
                    className={cn(
                      "flex items-center gap-1.5 px-3 py-1.5 rounded-full text-xs font-semibold transition-colors",
                      mode.key === snapshot?.current_mode
                        ? "bg-primary text-primary-foreground"
                        : "bg-secondary text-muted-foreground hover:bg-secondary/80 hover:text-foreground"
                    )}
                  >
                    <span>{mode.icon}</span>
                    {mode.label}
                  </button>
                ))}
              </div>
            </header>

            {/* Hero banner */}
            <div
              className="relative rounded-xl overflow-hidden mb-5"
              style={{
                minHeight: 260,
                background:
                  "radial-gradient(circle at 82% 50%, rgba(235,182,95,0.55), transparent 20%), radial-gradient(circle at 72% 50%, rgba(255,226,165,0.3), transparent 26%), linear-gradient(90deg, #070708 0%, #0c0a0f 54%, #18131a 100%)",
              }}
            >
              <div className="flex items-center justify-between h-full">
                {/* Left content */}
                <div className="p-8 flex-1">
                  <h2
                    className="text-2xl font-medium leading-tight tracking-tight text-[#f8f0db] mb-2"
                    style={{ fontFamily: "Georgia, 'Times New Roman', serif" }}
                  >
                    Make Said sound like you
                  </h2>
                  <p className="text-sm text-[#d5cec0] mb-6 max-w-xs leading-relaxed">
                    Set up different writing styles for different apps.
                  </p>

                  <div className="flex items-center gap-0">
                    <Button
                      onClick={onToggle}
                      disabled={isProcessing}
                      className={cn(
                        "rounded-xl text-sm font-semibold shadow-lg px-5 py-2.5 h-auto",
                        "bg-[#f7f2ea] text-[#3f3a38] hover:bg-white"
                      )}
                    >
                      {isProcessing
                        ? "Processing…"
                        : isRecording
                        ? "Stop now"
                        : "Start now"}
                    </Button>

                    {/* Recording orb */}
                    <div
                      className={cn(
                        "w-10 h-10 rounded-full -ml-3 flex-shrink-0 transition-colors duration-300",
                        isRecording
                          ? "orb-recording bg-[hsl(var(--recording))]"
                          : "bg-primary/70"
                      )}
                      style={{
                        border: isRecording
                          ? "5px solid hsl(var(--recording)/0.3)"
                          : "5px solid hsl(var(--primary)/0.3)",
                        boxShadow: isRecording
                          ? "0 0 0 8px hsl(var(--recording)/0.12)"
                          : "0 0 0 8px hsl(var(--primary)/0.1)",
                      }}
                    />
                  </div>
                </div>

                {/* Right decorative gradient */}
                <div
                  className="w-48 self-stretch flex-shrink-0"
                  style={{
                    background:
                      "radial-gradient(circle at 50% 48%, rgba(236,177,91,0.5), transparent 14%), radial-gradient(circle at 58% 56%, rgba(229,156,87,0.4), transparent 20%), radial-gradient(circle at 72% 44%, rgba(196,150,87,0.3), transparent 22%)",
                  }}
                />
              </div>
            </div>

            {/* Timeline */}
            {timeline.length === 0 ? (
              <div className="rounded-lg border border-border bg-card px-5 py-8 text-center">
                <p className="text-sm text-muted-foreground">
                  Your recordings will appear here. Press{" "}
                  <kbd className="rounded border border-border bg-secondary px-1.5 py-0.5 text-xs font-semibold">
                    Caps Lock
                  </kbd>{" "}
                  to start.
                </p>
              </div>
            ) : (
              <div className="space-y-5">
                {timeline.map((group) => (
                  <div key={group.label}>
                    <h3 className="text-[10px] font-extrabold tracking-widest text-muted-foreground mb-2 uppercase">
                      {group.label}
                    </h3>
                    <Card>
                      <CardContent className="p-0">
                        {group.items.map((item, idx) => (
                          <div
                            key={idx}
                            className={cn(
                              "grid gap-0",
                              idx > 0 && "border-t border-border"
                            )}
                            style={{ gridTemplateColumns: "120px 1fr" }}
                          >
                            <div className="px-4 py-3 text-xs text-muted-foreground font-medium flex-shrink-0">
                              {item.time}
                            </div>
                            <div className="px-4 py-3 text-sm text-foreground leading-relaxed border-l border-border">
                              {item.text || (
                                <span className="opacity-0">—</span>
                              )}
                            </div>
                          </div>
                        ))}
                      </CardContent>
                    </Card>
                  </div>
                ))}
              </div>
            )}
          </div>

          {/* ── RIGHT SIDEBAR ── */}
          <div className="flex flex-col gap-4">

            {/* Stats card */}
            <Card>
              <CardContent className="p-5 space-y-0">
                <div className="py-3 border-b border-border">
                  <div className="flex items-baseline gap-2">
                    <span
                      className="text-3xl font-bold tracking-tight"
                      style={{ fontFamily: "Georgia, 'Times New Roman', serif" }}
                    >
                      {(snapshot?.total_words ?? 0).toLocaleString()}
                    </span>
                    <span className="text-sm text-muted-foreground">
                      total words
                    </span>
                  </div>
                </div>

                <div className="py-3 border-b border-border">
                  <div className="flex items-baseline gap-2">
                    <span
                      className="text-3xl font-bold tracking-tight"
                      style={{ fontFamily: "Georgia, 'Times New Roman', serif" }}
                    >
                      {snapshot?.avg_wpm ?? 0}
                    </span>
                    <span className="text-sm text-muted-foreground">wpm</span>
                  </div>
                </div>

                <div className="py-3">
                  <div className="flex items-baseline gap-2">
                    <span
                      className="text-3xl font-bold tracking-tight"
                      style={{ fontFamily: "Georgia, 'Times New Roman', serif" }}
                    >
                      {snapshot?.daily_streak ?? 0}
                    </span>
                    <span className="text-sm text-muted-foreground">
                      day streak
                    </span>
                  </div>
                </div>
              </CardContent>
            </Card>

            {/* Voice profile card */}
            <Card>
              <CardContent className="p-5">
                <h3 className="text-sm font-semibold text-foreground mb-1">
                  Your Voice Profile
                </h3>
                <p className="text-xs text-muted-foreground mb-3 leading-relaxed">
                  Discover how you use your voice.
                </p>
                <Progress value={profilePct} className="h-1.5 mb-3" />
                <div className="flex items-center justify-between gap-2">
                  <span className="text-xs text-muted-foreground">
                    {snapshot?.accessibility_granted
                      ? "Auto-paste ready"
                      : `Unlock in ${wordsLeft}`}
                  </span>
                  <button
                    onClick={onAccessibility}
                    disabled={!snapshot?.auto_paste_supported}
                    className="text-xs font-semibold text-primary hover:text-primary/80 transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
                  >
                    {snapshot?.accessibility_granted
                      ? "Connected"
                      : "Enable access"}
                  </button>
                </div>
              </CardContent>
            </Card>

            {/* Latest polish card */}
            <Card>
              <CardContent className="p-5">
                <div className="text-[10px] font-extrabold tracking-widest text-muted-foreground uppercase mb-2">
                  Latest Polish
                </div>
                <p className="text-sm text-foreground leading-relaxed mb-3">
                  {result?.polished ??
                    "Your cleaned-up writing will appear here after the first recording."}
                </p>
                <div className="flex items-center justify-between gap-2 text-xs text-muted-foreground">
                  <span
                    className={cn(
                      "font-medium",
                      isRecording && "text-[hsl(var(--recording))]",
                      isProcessing && "text-[hsl(var(--amber))]"
                    )}
                  >
                    {statusLabel(snapshot, busy)}
                  </span>
                  <span>{result?.model ?? snapshot?.current_model ?? "—"}</span>
                </div>
              </CardContent>
            </Card>
          </div>
        </div>
      </div>
    </ScrollArea>
  );
}
