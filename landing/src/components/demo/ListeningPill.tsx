"use client";

import { cn } from "@/lib/cn";

type Props = {
  className?: string;
  animate?: boolean;
};

/**
 * Black floating capsule that visually represents Airnote "listening".
 * A small live-mic dot pulses red; a 4-bar mini waveform animates with
 * staggered wfBar keyframes (defined in globals.css).
 */
export function ListeningPill({ className, animate = true }: Props) {
  return (
    <div
      className={cn(
        "inline-flex items-center gap-2 h-9 px-3 rounded-full",
        "bg-[#0a0a0b]/95 border border-white/8",
        "shadow-[0_8px_24px_-8px_rgba(0,0,0,0.6)]",
        className,
      )}
    >
      <span
        className={cn(
          "h-1.5 w-1.5 rounded-full bg-red-400",
          animate && "animate-pulse",
        )}
      />
      <div className="flex items-end gap-[2px] h-4">
        {Array.from({ length: 5 }).map((_, i) => (
          <span
            key={i}
            className={
              animate
                ? "w-[2px] rounded-full bg-accent animate-wfBar"
                : "w-[2px] rounded-full bg-accent"
            }
            style={{
              height: "100%",
              animationDelay: animate ? `${i * 0.09}s` : undefined,
              transform: animate ? undefined : "scaleY(0.5)",
              transformOrigin: "bottom",
            }}
          />
        ))}
      </div>
    </div>
  );
}
