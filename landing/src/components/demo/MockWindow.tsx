"use client";

import { cn } from "@/lib/cn";

type Props = {
  className?: string;
  title?: string;
  children: React.ReactNode;
  /** Suppress the centered title text in the chrome. */
  blankTitle?: boolean;
};

/**
 * Generic macOS-style window chrome used by every demo app mock.
 * Renders a 28px title bar with three traffic-light dots flush-left,
 * an optional centered title, and a body for the caller's content.
 */
export function MockWindow({ className, title, children, blankTitle }: Props) {
  return (
    <div
      className={cn(
        "relative flex flex-col rounded-xl overflow-hidden",
        "shadow-[0_40px_60px_-20px_rgba(0,0,0,0.55),0_12px_24px_-8px_rgba(0,0,0,0.4)]",
        "border border-black/20",
        className,
      )}
    >
      <div className="relative flex h-7 items-center px-3 shrink-0 bg-[#e9e9eb] border-b border-black/8">
        <div className="flex items-center gap-1.5">
          <span className="h-2.5 w-2.5 rounded-full bg-[#ff5f57]" />
          <span className="h-2.5 w-2.5 rounded-full bg-[#febc2e]" />
          <span className="h-2.5 w-2.5 rounded-full bg-[#28c840]" />
        </div>
        {title && !blankTitle && (
          <span className="absolute left-1/2 -translate-x-1/2 text-[11px] text-[#7a7a80] font-medium tracking-tight">
            {title}
          </span>
        )}
      </div>
      <div className="relative flex-1 overflow-hidden">{children}</div>
    </div>
  );
}
