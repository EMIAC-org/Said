"use client";

import { cn } from "@/lib/cn";
import { MockWindow } from "./MockWindow";
import { heroDemo } from "@/lib/content";

const CODE_PALETTE = ["#ff79c6", "#f1fa8c", "#8be9fd", "#f8f8f2", "#6272a4"];

/** Deterministic "random" so SSR and CSR produce the same markup. */
function bar(i: number): { color: string; width: number; pad: number } {
  // Simple LCG-ish pattern from index, no actual randomness.
  const seed = (i * 9301 + 49297) % 233280;
  const color = CODE_PALETTE[(seed >> 4) % CODE_PALETTE.length];
  const width = 35 + (seed % 60); // 35–95 %
  const pad = (i % 6) * 8;        // hanging indent suggestion
  return { color, width, pad };
}

export function MockCursorWindow({ className }: { className?: string }) {
  const { cursor } = heroDemo;
  return (
    <MockWindow className={cn("text-[#d1d1d6]", className)} blankTitle>
      <div className="flex flex-col h-full bg-[#181818]">
        {/* Tab bar */}
        <div className="flex items-center h-6 px-2 gap-1 bg-[#0e0e10] border-b border-black/40">
          <span className="inline-flex items-center gap-1 px-2 py-0.5 text-[9px] rounded-sm bg-[#1f1f23] text-[#d1d1d6]">
            <span className="h-1.5 w-1.5 rounded-full bg-[#ff79c6]" />
            {cursor.tabName}
          </span>
        </div>
        {/* Breadcrumb */}
        <div className="px-3 py-1 text-[9px] text-[#7a7a85] border-b border-black/40">
          {cursor.breadcrumb}
        </div>

        <div className="flex flex-1 min-h-0">
          {/* Code area */}
          <div className="flex-1 overflow-hidden p-2.5 pl-4">
            <div className="flex">
              {/* Line numbers */}
              <div className="text-[7px] text-[#3a3a40] pr-2 font-mono leading-[10px] select-none">
                {Array.from({ length: 22 }, (_, i) => (
                  <div key={i}>{45 + i}</div>
                ))}
              </div>
              {/* Bars */}
              <div className="flex-1 space-y-[2px]">
                {Array.from({ length: 22 }).map((_, i) => {
                  const { color, width, pad } = bar(i);
                  return (
                    <div key={i} className="flex items-center h-[7px]">
                      <div style={{ width: pad }} />
                      <div
                        className="h-[3px] rounded-full"
                        style={{
                          width: `${width}%`,
                          background: color,
                          opacity: 0.6,
                        }}
                      />
                    </div>
                  );
                })}
              </div>
            </div>
          </div>

          {/* Right chat panel */}
          <aside className="w-[38%] shrink-0 border-l border-black/40 bg-[#1c1c1f] flex flex-col">
            <div className="px-2 py-2 border-b border-black/30">
              <div className="inline-flex items-center gap-1 text-[9px] px-1.5 py-0.5 rounded bg-[#0e0e10] border border-white/8">
                <span className="text-[#8be9fd]">+</span>
                <span className="font-mono text-[#d1d1d6]">
                  {cursor.chatFile}
                </span>
                <span className="text-[#7a7a85]">{cursor.chatFileNote}</span>
              </div>
            </div>
            <div className="flex-1 p-2 text-[11px] leading-snug text-[#d1d1d6]">
              {cursor.promptText}
            </div>
            <div className="px-2 pb-2 flex items-center gap-1.5 text-[8px] text-[#7a7a85]">
              <span className="inline-flex items-center gap-0.5">
                <kbd>⌃</kbd> {cursor.modelChip}
              </span>
              <span className="ml-auto inline-flex items-center gap-0.5">
                <kbd>↵</kbd> {cursor.modeChip}
              </span>
              <span className="inline-flex items-center gap-0.5">
                <kbd>⌘</kbd>
                <kbd>↵</kbd>
                {cursor.contextChip}
              </span>
            </div>
          </aside>
        </div>
      </div>
    </MockWindow>
  );
}
