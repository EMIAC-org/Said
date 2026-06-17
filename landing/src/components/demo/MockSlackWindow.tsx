"use client";

import { cn } from "@/lib/cn";
import { MockWindow } from "./MockWindow";
import { heroDemo } from "@/lib/content";

export function MockSlackWindow({ className }: { className?: string }) {
  const { slack } = heroDemo;
  return (
    <MockWindow className={cn("text-[#1d1d1f]", className)}>
      <div className="flex h-full">
        {/* Activity rail */}
        <div className="w-9 shrink-0 bg-[#3F0F40] flex flex-col items-center gap-3 pt-3">
          <span className="h-6 w-6 rounded-md bg-white/15" />
          <span className="h-1.5 w-1.5 rounded-full bg-white/40" />
          <span className="h-1.5 w-1.5 rounded-full bg-white/40" />
          <span className="h-1.5 w-1.5 rounded-full bg-white/40" />
        </div>

        {/* Channel sidebar */}
        <aside className="w-[34%] shrink-0 bg-[#F4EDF4] p-2 border-r border-[#e6dbe6]">
          <div className="flex items-center justify-between px-1 py-1">
            <div className="text-[11px] font-semibold text-[#1d1d1f]">
              {slack.teamLabel}
            </div>
          </div>
          <div className="mt-2 space-y-0.5 text-[10px] text-[#3f0e40]">
            <div className="px-1.5 py-0.5">Unreads</div>
            <div className="px-1.5 py-0.5">Drafts and sent</div>
          </div>
          <div className="mt-3 px-1.5 text-[9px] font-medium text-[#3f0e40]/70 uppercase tracking-wide">
            Channels
          </div>
          <div className="mt-1 space-y-0.5 text-[10px]">
            <div className="px-1.5 py-0.5 rounded text-[#3f0e40]"># announcements</div>
            <div className="px-1.5 py-0.5 rounded bg-[#1164a3] text-white font-medium">
              # standup
            </div>
          </div>
          <div className="mt-3 px-1.5 text-[9px] font-medium text-[#3f0e40]/70 uppercase tracking-wide">
            Direct messages
          </div>
          <div className="mt-1 space-y-0.5 text-[10px] text-[#3f0e40]">
            <div className="flex items-center justify-between px-1.5 py-0.5">
              <span className="flex items-center gap-1">
                <span className="h-2 w-2 rounded-full bg-green-400" />
                Lisa
              </span>
              <span className="rounded-full bg-[#cd2553] text-white text-[8px] px-1.5 leading-tight">1</span>
            </div>
            <div className="flex items-center justify-between px-1.5 py-0.5">
              <span className="flex items-center gap-1">
                <span className="h-2 w-2 rounded-full bg-gray-400" />
                Dario
              </span>
              <span className="rounded-full bg-[#cd2553] text-white text-[8px] px-1.5 leading-tight">2</span>
            </div>
          </div>
        </aside>

        {/* Channel content */}
        <section className="flex-1 flex flex-col bg-white">
          <div className="border-b border-[#eee] px-3 py-2 text-[11px] font-semibold text-[#1d1d1f]">
            {slack.channelLabel}
          </div>
          <div className="flex-1 p-3 overflow-hidden">
            <div className="text-[11px] font-semibold text-[#1d1d1f] mb-1">
              {slack.welcomeTitle}
            </div>
            <div className="text-[10px] text-[#5a5a60] leading-snug mb-3">
              {slack.welcomeBody}
            </div>
            <div className="flex items-center gap-2 my-2">
              <span className="flex-1 h-px bg-[#e8e8e8]" />
              <span className="text-[9px] text-[#9a9aa0]">{slack.todayDivider}</span>
              <span className="flex-1 h-px bg-[#e8e8e8]" />
            </div>
            <div className="flex items-start gap-2 mt-2">
              <span
                aria-hidden
                className="h-6 w-6 rounded-md grid place-items-center text-[10px] font-semibold text-white shrink-0"
                style={{
                  background:
                    "linear-gradient(135deg, #c47e6a, #8a4a3c)",
                  boxShadow: "inset 0 1px 0 rgba(255,255,255,0.2)",
                }}
              >
                {slack.messageAuthor.charAt(0)}
              </span>
              <div className="min-w-0">
                <div className="flex items-baseline gap-1.5">
                  <span className="text-[10px] font-semibold text-[#1d1d1f]">
                    {slack.messageAuthor}
                  </span>
                  <span className="text-[9px] text-[#8a8a90]">
                    {slack.messageTime}
                  </span>
                </div>
                <div className="text-[10px] text-[#1d1d1f] leading-snug">
                  {slack.messageBody}
                </div>
              </div>
            </div>
          </div>
          <div className="border-t border-[#eee] px-3 py-2">
            <div className="h-6 rounded bg-[#f6f6f8] border border-[#e6e6ea] flex items-center px-2 text-[9px] text-[#9a9aa0]">
              {slack.inputPlaceholder}
            </div>
          </div>
        </section>
      </div>
    </MockWindow>
  );
}
