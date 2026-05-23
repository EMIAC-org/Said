import { Loader2, CheckCircle2 } from "lucide-react";
import type { HealthLevel } from "@/lib/useBackendHeartbeat";

interface Props {
  level: HealthLevel;
  showOverlay: boolean;
  justRecovered: boolean;
}

export function ReconnectingOverlay({ level, showOverlay, justRecovered }: Props) {
  if (justRecovered) {
    return (
      <div className="fixed inset-x-0 bottom-6 z-[9999] flex justify-center pointer-events-none">
        <div
          className="flex items-center gap-2 px-4 py-2.5 rounded-xl pointer-events-auto"
          style={{
            background: "hsl(152 60% 40% / 0.18)",
            backdropFilter: "blur(16px)",
            boxShadow: "inset 0 0 0 1px hsl(152 60% 50% / 0.25), 0 8px 24px hsl(0 0% 0% / 0.3)",
          }}
        >
          <CheckCircle2 size={15} style={{ color: "hsl(152 60% 65%)" }} />
          <span className="text-[12.5px] font-medium" style={{ color: "hsl(152 60% 80%)" }}>
            Reconnected
          </span>
        </div>
      </div>
    );
  }

  if (!showOverlay) return null;

  return (
    <div className="fixed inset-0 z-[9999] flex items-center justify-center" style={{ background: "hsl(0 0% 0% / 0.45)" }}>
      <div
        className="flex flex-col items-center gap-3 px-8 py-6 rounded-2xl"
        style={{
          background: "hsl(var(--card) / 0.95)",
          backdropFilter: "blur(24px)",
          boxShadow: "var(--shadow-glass)",
        }}
      >
        <Loader2 size={22} className="animate-spin" style={{ color: "hsl(var(--primary))" }} />
        <div className="text-center">
          <p className="text-[14px] font-semibold" style={{ color: "hsl(var(--foreground))" }}>
            {level === "unreachable" ? "Reconnecting…" : "AirNote is recovering…"}
          </p>
          <p className="text-[11.5px] mt-1" style={{ color: "hsl(var(--muted-foreground))" }}>
            Please wait, this should only take a moment.
          </p>
        </div>
      </div>
    </div>
  );
}
