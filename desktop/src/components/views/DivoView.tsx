import { useCallback, useEffect, useState } from "react";
import { Sparkles, Lock, Search, Plus, Loader2, Mic } from "lucide-react";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Markdown } from "@/components/Markdown";
import {
  divoListThreads,
  divoThreadMessages,
  divoSetActiveThread,
  type DivoThreadSummary,
  type DivoThreadDetail,
} from "@/lib/invoke";
import { formatKeycap, type Platform } from "@/lib/hotkeys";

// Relative time for the chat list ("now", "2m", "3h", "Yesterday", "Mon 2").
function relTime(iso?: string | null): string {
  if (!iso) return "";
  const t = new Date(iso).getTime();
  if (!Number.isFinite(t)) return "";
  const diff = Date.now() - t;
  const m = Math.floor(diff / 60_000);
  if (m < 1) return "now";
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h`;
  const d = Math.floor(h / 24);
  if (d === 1) return "Yesterday";
  if (d < 7) return `${d}d`;
  return new Date(t).toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

function Kbd({ children }: { children: React.ReactNode }) {
  return (
    <span
      className="font-mono text-[10.5px] px-1.5 py-0.5 rounded"
      style={{ background: "hsl(var(--surface-4))", border: "1px solid hsl(var(--border))" }}
    >
      {children}
    </span>
  );
}

export function DivoView({ platform }: { platform?: string }) {
  // Default to macOS glyphs when platform is unknown (preserves prior behavior).
  const plat = (platform ?? "macos") as Platform;
  const ctrl = formatKeycap("ctrl", plat); // "⌃" on macOS, "Ctrl" on Windows
  const ctrlN = formatKeycap("ctrl+n", plat); // "⌃N" / "Ctrl+N"
  const [threads, setThreads] = useState<DivoThreadSummary[]>([]);
  const [loadingList, setLoadingList] = useState(true);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [detail, setDetail] = useState<DivoThreadDetail | null>(null);
  const [loadingDetail, setLoadingDetail] = useState(false);
  const [query, setQuery] = useState("");

  const openChat = useCallback(async (id: string) => {
    setActiveId(id);
    setLoadingDetail(true);
    void divoSetActiveThread(id); // a plain ⌃ hold now continues this chat
    const d = await divoThreadMessages(id);
    setDetail(d);
    setLoadingDetail(false);
  }, []);

  const refreshList = useCallback(async () => {
    setLoadingList(true);
    const list = await divoListThreads();
    setThreads(list);
    setLoadingList(false);
    return list;
  }, []);

  useEffect(() => {
    void (async () => {
      const list = await refreshList();
      if (list.length > 0) await openChat(list[0].id);
    })();
  }, [refreshList, openChat]);

  const newChat = useCallback(() => {
    setActiveId(null);
    setDetail(null);
    void divoSetActiveThread(null); // next ⌃ starts fresh (same as ⌃N)
  }, []);

  const q = query.trim().toLowerCase();
  const filtered = q
    ? threads.filter(
        (t) =>
          (t.title || "").toLowerCase().includes(q) ||
          (t.preview || "").toLowerCase().includes(q),
      )
    : threads;

  const activeTitle = detail?.title || threads.find((t) => t.id === activeId)?.title || "New chat";

  return (
    <div className="h-full flex flex-col" style={{ background: "hsl(var(--surface-2))" }}>
      {/* ── Header ─────────────────────────────────────────────── */}
      <div
        className="flex items-center gap-3 px-6 py-4 flex-shrink-0"
        style={{ background: "hsl(var(--surface-3))", borderBottom: "1px solid hsl(var(--border))" }}
      >
        <span
          className="grid place-items-center w-7 h-7 rounded-lg flex-shrink-0"
          style={{
            background: "linear-gradient(135deg, hsl(226 80% 76%), hsl(280 70% 74%))",
            color: "hsl(240 8% 8%)",
            boxShadow: "0 0 14px hsl(226 80% 70% / 0.4)",
          }}
        >
          <Sparkles size={15} strokeWidth={2.2} />
        </span>
        <div className="min-w-0">
          <h1 className="text-[15px] font-bold leading-tight text-foreground">Divo</h1>
          <p className="text-[11px] text-muted-foreground leading-tight">
            Your AirNote&nbsp;⇄&nbsp;Divo chats
          </p>
        </div>
        <div className="ml-auto flex items-center gap-3 flex-shrink-0">
          <span className="hidden md:flex items-center gap-2 text-[11px] text-muted-foreground">
            <span className="flex items-center gap-1"><Kbd>{ctrl}</Kbd> same chat</span>
            <span className="flex items-center gap-1"><Kbd>{ctrlN}</Kbd> new chat</span>
          </span>
          <span
            className="inline-flex items-center gap-1.5 text-[11px] font-semibold px-2.5 py-1 rounded-lg"
            style={{
              color: "hsl(var(--chip-amber-fg))",
              background: "hsl(var(--chip-amber-bg))",
            }}
            title="The Divo channel is limited to approved EMIAC accounts"
          >
            <Lock size={11} /> Approved accounts only
          </span>
        </div>
      </div>

      {/* ── Body ──────────────────────────────────────────────── */}
      <div className="flex-1 flex min-h-0">
        {/* Chat history list */}
        <div
          className="w-[280px] flex-shrink-0 flex flex-col min-h-0"
          style={{ background: "hsl(var(--surface-2))", borderRight: "1px solid hsl(var(--border))" }}
        >
          <div className="flex items-center gap-2 px-4 pt-4 pb-2 flex-shrink-0">
            <span className="text-[13px] font-bold text-foreground">Chats</span>
            <button
              className="ml-auto inline-flex items-center gap-1 text-[11.5px] font-semibold px-2.5 py-1 rounded-md transition-colors"
              style={{ color: "hsl(var(--primary-foreground))", background: "hsl(var(--primary))" }}
              onClick={newChat}
            >
              <Plus size={12} strokeWidth={2.6} /> New
            </button>
          </div>

          <div className="px-4 pb-2 flex-shrink-0">
            <div
              className="flex items-center gap-2 h-8 px-2.5 rounded-lg"
              style={{ background: "hsl(var(--input))", border: "1px solid hsl(var(--border))" }}
            >
              <Search size={13} className="text-muted-foreground flex-shrink-0" />
              <input
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder="Search chats…"
                className="flex-1 bg-transparent outline-none text-[12px] text-foreground placeholder:text-muted-foreground/70"
              />
            </div>
          </div>

          <ScrollArea className="flex-1 min-h-0">
            <div className="px-2 pb-3 flex flex-col gap-0.5">
              {loadingList ? (
                <div className="flex items-center justify-center gap-2 py-8 text-[12px] text-muted-foreground">
                  <Loader2 size={14} className="animate-spin" /> Loading…
                </div>
              ) : filtered.length === 0 ? (
                <div className="px-3 py-8 text-center text-[12px] text-muted-foreground leading-relaxed">
                  {threads.length === 0
                    ? `No chats yet.\nHold ${ctrl} and speak to start one.`
                    : "No chats match your search."}
                </div>
              ) : (
                filtered.map((t) => (
                  <button
                    key={t.id}
                    onClick={() => void openChat(t.id)}
                    className="text-left px-3 py-2.5 rounded-lg transition-colors"
                    style={
                      t.id === activeId
                        ? { background: "hsl(var(--primary) / 0.10)", border: "1px solid hsl(var(--primary) / 0.24)" }
                        : { border: "1px solid transparent" }
                    }
                  >
                    <div className="flex items-center gap-2">
                      <span className="flex-1 truncate text-[12.5px] font-semibold text-foreground">
                        {t.title || "Untitled chat"}
                      </span>
                      <span className="flex-shrink-0 text-[10px] text-muted-foreground tabular-nums">
                        {relTime(t.lastMessageAt ?? t.updatedAt ?? t.createdAt)}
                      </span>
                    </div>
                    {t.preview ? (
                      <p className="mt-0.5 truncate text-[11px] text-muted-foreground">{t.preview}</p>
                    ) : null}
                  </button>
                ))
              )}
            </div>
          </ScrollArea>
        </div>

        {/* Conversation pane */}
        <div className="flex-1 flex flex-col min-w-0 min-h-0">
          <div
            className="flex items-center gap-2 px-5 py-3 flex-shrink-0"
            style={{ background: "hsl(var(--surface-3))", borderBottom: "1px solid hsl(var(--border))" }}
          >
            <span className="truncate text-[13.5px] font-bold text-foreground">{activeTitle}</span>
          </div>

          <ScrollArea className="flex-1 min-h-0">
            <div className="px-5 py-5 flex flex-col gap-5 max-w-[680px] mx-auto">
              {loadingDetail ? (
                <div className="flex items-center justify-center gap-2 py-10 text-[12.5px] text-muted-foreground">
                  <Loader2 size={15} className="animate-spin" /> Loading conversation…
                </div>
              ) : !detail || detail.messages.length === 0 ? (
                <div className="flex flex-col items-center gap-2 py-16 text-center text-muted-foreground">
                  <span
                    className="grid place-items-center w-11 h-11 rounded-xl mb-1"
                    style={{ background: "hsl(var(--surface-4))", border: "1px solid hsl(var(--border))" }}
                  >
                    <Mic size={18} />
                  </span>
                  <p className="text-[13px] font-medium text-foreground">
                    {activeId ? "No messages yet" : "Start a new chat"}
                  </p>
                  <p className="text-[12px] leading-relaxed">
                    Hold <Kbd>{ctrl}</Kbd> and speak to {activeId ? "continue this chat" : "begin"}.
                  </p>
                </div>
              ) : (
                detail.messages.map((m) =>
                  m.role === "user" ? (
                    <div key={m.id} className="flex justify-end">
                      <div
                        className="max-w-[80%] px-3.5 py-2 text-[13px] leading-relaxed"
                        style={{
                          color: "hsl(var(--foreground) / 0.97)",
                          background: "hsl(var(--primary) / 0.14)",
                          border: "1px solid hsl(var(--primary) / 0.28)",
                          borderRadius: "14px 14px 4px 14px",
                        }}
                      >
                        {m.content}
                      </div>
                    </div>
                  ) : (
                    <div key={m.id} className="flex gap-3">
                      <span
                        className="grid place-items-center w-6 h-6 rounded-md flex-shrink-0 mt-0.5"
                        style={{
                          background: "linear-gradient(135deg, hsl(226 80% 76%), hsl(280 70% 74%))",
                          color: "hsl(240 8% 8%)",
                        }}
                      >
                        <Sparkles size={12} strokeWidth={2.2} />
                      </span>
                      <div className="flex-1 min-w-0 text-[13px] leading-relaxed text-foreground/90 divo-md">
                        <Markdown content={m.content} />
                      </div>
                    </div>
                  ),
                )
              )}
            </div>
          </ScrollArea>

          <div
            className="flex items-center gap-2 px-5 py-3 flex-shrink-0 text-[11.5px] text-muted-foreground"
            style={{ background: "hsl(var(--surface-3))", borderTop: "1px solid hsl(var(--border))" }}
          >
            <Mic size={13} className="flex-shrink-0" />
            Hold <Kbd>{ctrl}</Kbd> to continue this chat&nbsp;·&nbsp;<Kbd>{ctrlN}</Kbd> to start a new one
          </div>
        </div>
      </div>
    </div>
  );
}
