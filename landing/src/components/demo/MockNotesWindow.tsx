"use client";

import { cn } from "@/lib/cn";
import { MockWindow } from "./MockWindow";
import { heroDemo } from "@/lib/content";

type Props = {
  className?: string;
  /** Number of chars to reveal in the editor body (typewriter offset). */
  bodyChars?: number;
  /** Show the blinking caret at the end of the typed text. */
  showCaret?: boolean;
};

export function MockNotesWindow({
  className,
  bodyChars = 0,
  showCaret = true,
}: Props) {
  const { notes } = heroDemo;
  const text = notes.body.slice(0, bodyChars);
  const lines = text.split("\n");

  return (
    <MockWindow className={cn("text-[#1d1d1f]", className)}>
      <div className="flex h-full bg-white">
        {/* Sidebar */}
        <aside className="w-[36%] shrink-0 bg-[#f3f3f5] border-r border-[#e5e5e7] p-2">
          <div className="px-2 py-1 text-[10px] font-medium text-[#3a3a3c]">
            {notes.today}
          </div>
          <ul className="mt-1 space-y-0.5">
            {notes.items.map((item, i) => (
              <li
                key={item.title}
                className={cn(
                  "rounded-md px-2 py-1.5",
                  i === 0
                    ? "bg-[#FFD968]"
                    : "hover:bg-black/5 transition-colors",
                )}
              >
                <div className="text-[10px] font-medium leading-tight truncate">
                  {item.title}
                </div>
                <div className="mt-0.5 flex items-center gap-1 text-[9px] text-[#6b6b70] leading-tight">
                  <span className="font-medium">{item.time}</span>
                  <span className="truncate">{item.preview}</span>
                </div>
              </li>
            ))}
          </ul>
        </aside>

        {/* Editor */}
        <section className="flex-1 flex flex-col bg-white">
          <div className="py-2 text-center text-[9px] text-[#8a8a90] border-b border-[#f0f0f2]">
            {notes.dateHeader}
          </div>
          <div className="flex-1 p-3 text-[12px] leading-relaxed whitespace-pre-line">
            {lines.map((line, i) => (
              <div key={i}>
                {line || " "}
                {showCaret && i === lines.length - 1 && (
                  <span
                    className="inline-block w-[1px] h-[1em] align-middle bg-[#1d1d1f] animate-caret ml-[1px]"
                  />
                )}
              </div>
            ))}
          </div>
        </section>
      </div>
    </MockWindow>
  );
}
