import { useCallback, useEffect, useMemo, useState } from "react";
import { ArrowRight, BookOpen, Plus, RotateCcw, Search, Trash2, X } from "lucide-react";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Skeleton } from "@/components/ui/skeleton";
import {
  addDictionaryWord,
  clearDictionary,
  deleteDictionaryWord,
  listDictionary,
  onDictionaryChanged,
  type DictionaryWord,
} from "@/lib/invoke";
import { friendlyError } from "@/lib/friendlyError";

const MAX_LEN = 80;

function relativeTime(ms: number): string {
  if (!ms || ms <= 0) return "";
  const diff = Date.now() - ms;
  if (diff < 45_000) return "just now";
  const m = Math.floor(diff / 60_000);
  if (m < 60) return `${m}m ago`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ago`;
  const d = Math.floor(h / 24);
  if (d === 1) return "yesterday";
  if (d < 30) return `${d}d ago`;
  return new Date(ms).toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

function WordRow({ word, onDelete }: { word: DictionaryWord; onDelete: (w: DictionaryWord) => void }) {
  return (
    <div className="vocab-row group">
      <div className="flex items-center gap-3 pl-4 pr-2 min-h-[44px]">
        <div className="flex-1 min-w-0 flex items-center gap-2 text-[13px]">
          {word.heard ? (
            <>
              <span className="text-muted-foreground truncate">{word.heard}</span>
              <ArrowRight size={11} className="text-muted-foreground flex-shrink-0" />
            </>
          ) : null}
          <span className="font-medium text-foreground truncate">{word.written}</span>
        </div>
        <span className="w-20 text-right text-[12px] text-muted-foreground">
          {word.source === "learned" ? "Learned" : "Added"}
        </span>
        <span className="w-20 text-right text-[12px] text-muted-foreground tabular-nums">
          {relativeTime(word.created_at)}
        </span>
        <button
          onClick={() => onDelete(word)}
          title="Delete"
          aria-label={`Delete ${word.written}`}
          className="w-7 h-7 rounded-md flex items-center justify-center text-muted-foreground opacity-0 group-hover:opacity-100 focus:opacity-100 transition-opacity hover:bg-[hsl(var(--foreground)/0.06)] hover:text-[hsl(var(--destructive))]"
        >
          <Trash2 size={13} />
        </button>
      </div>
    </div>
  );
}

export function DictionaryView() {
  const [words, setWords] = useState<DictionaryWord[] | null>(null);
  const [written, setWritten] = useState("");
  const [heard, setHeard] = useState("");
  const [adding, setAdding] = useState(false);
  const [search, setSearch] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [confirmClear, setConfirmClear] = useState(false);

  const refresh = useCallback(async () => {
    try {
      setWords(await listDictionary());
    } catch (err) {
      setWords((prev) => prev ?? []);
      setError(friendlyError(err));
    }
  }, []);

  useEffect(() => {
    void refresh();
    return onDictionaryChanged(() => void refresh());
  }, [refresh]);

  async function handleAdd() {
    const w = written.trim();
    const h = heard.trim();
    if (!w || adding) return;
    setAdding(true);
    setError(null);
    try {
      await addDictionaryWord(w, h || null);
      setWritten("");
      setHeard("");
      await refresh();
    } catch (err) {
      setError(friendlyError(err));
    } finally {
      setAdding(false);
    }
  }

  async function handleDelete(word: DictionaryWord) {
    setWords((prev) => prev?.filter((w) => w.id !== word.id) ?? prev);
    try {
      await deleteDictionaryWord(word.id);
    } catch (err) {
      setError(friendlyError(err));
      await refresh();
    }
  }

  async function handleClear() {
    setConfirmClear(false);
    try {
      await clearDictionary();
      setWords([]);
    } catch (err) {
      setError(friendlyError(err));
    }
  }

  const q = search.trim().toLowerCase();
  const shown = useMemo(
    () => (words ?? []).filter((w) =>
      !q || w.written.toLowerCase().includes(q) || (w.heard ?? "").toLowerCase().includes(q)),
    [words, q],
  );

  return (
    <ScrollArea className="h-full">
      <div className="p-7 pb-12 max-w-3xl mx-auto">
        <div className="mb-5 flex items-start justify-between gap-4">
          <div>
            <h1 className="text-[28px] font-bold tracking-tight text-foreground leading-tight">Dictionary</h1>
            <p className="text-[13px] text-muted-foreground mt-1">
              Words polish writes your way. When a dictation contains a word on the left, polish writes the one on the right. With polish off, the list isn't used.
            </p>
          </div>
          {(words?.length ?? 0) > 0 && (
            <button
              onClick={() => setConfirmClear(true)}
              className="flex items-center gap-1.5 mt-2 text-[12px] font-medium px-2.5 h-8 rounded-lg text-muted-foreground hover:text-[hsl(var(--destructive))] hover:bg-[hsl(var(--destructive)/0.06)] transition-colors flex-shrink-0"
            >
              <RotateCcw size={12.5} /> Clear all
            </button>
          )}
        </div>

        <div className="flex items-center gap-2 mb-2">
          <div className="field flex-1 h-10">
            <input
              value={heard}
              maxLength={MAX_LEN}
              onChange={(e) => setHeard(e.target.value)}
              onKeyDown={(e) => { if (e.key === "Enter") void handleAdd(); }}
              placeholder="What AirNote hears (optional), e.g. air note"
              className="text-[13px]"
            />
          </div>
          <ArrowRight size={14} className="text-muted-foreground flex-shrink-0" />
          <div className="field flex-1 h-10">
            <Plus size={15} className="text-muted-foreground flex-shrink-0" />
            <input
              value={written}
              maxLength={MAX_LEN}
              onChange={(e) => setWritten(e.target.value)}
              onKeyDown={(e) => { if (e.key === "Enter") void handleAdd(); }}
              placeholder="How to write it, e.g. AirNote"
              className="text-[13px]"
            />
          </div>
          <button
            onClick={() => void handleAdd()}
            disabled={!written.trim() || adding}
            className="btn-primary flex-shrink-0"
            style={{ height: 40, minWidth: 72 }}
          >
            {adding ? "Adding…" : "Add"}
          </button>
        </div>
        <p className="text-[12px] text-muted-foreground mb-6 px-1">
          AirNote also learns here on its own: when you fix a name it typed, the fix is added and you see “Learned” with an Undo.
        </p>

        {error && (
          <div className="mb-4 px-3 py-2 rounded-lg text-[12px] flex items-center gap-2" style={{ background: "hsl(var(--destructive) / 0.08)", color: "hsl(var(--destructive))" }}>
            <span className="flex-1">{error}</span>
            <button onClick={() => setError(null)} aria-label="Dismiss"><X size={12} /></button>
          </div>
        )}

        {confirmClear && (
          <div className="mb-5 p-4 rounded-lg border" style={{ background: "hsl(var(--destructive) / 0.06)", borderColor: "hsl(var(--destructive) / 0.25)" }}>
            <p className="text-[13px] font-semibold text-foreground mb-1">Delete all {words?.length} words?</p>
            <p className="text-[12px] text-muted-foreground mb-3">This can't be undone. History and settings are not affected.</p>
            <div className="flex gap-2">
              <button onClick={() => void handleClear()} className="text-[11px] font-semibold px-3 py-1.5 rounded-md" style={{ background: "hsl(var(--destructive))", color: "white" }}>
                Delete all
              </button>
              <button onClick={() => setConfirmClear(false)} className="text-[11px] font-medium px-3 py-1.5 rounded-md" style={{ background: "hsl(var(--surface-2))", color: "hsl(var(--foreground))" }}>
                Cancel
              </button>
            </div>
          </div>
        )}

        {words === null ? (
          <div className="vocab-list">
            {Array.from({ length: 6 }).map((_, i) => (
              <div key={i} className="vocab-row flex items-center gap-3 pl-4 pr-4 h-[44px]">
                <Skeleton className="h-3" style={{ width: `${34 - i * 3}%` }} />
                <span className="flex-1" />
                <Skeleton className="h-2.5 w-24" />
              </div>
            ))}
          </div>
        ) : words.length === 0 ? (
          <div className="flex items-center justify-center py-16">
            <div className="text-center px-8">
              <div className="w-12 h-12 rounded-full flex items-center justify-center mx-auto mb-4" style={{ background: "hsl(var(--primary) / 0.1)" }}>
                <BookOpen size={20} style={{ color: "hsl(var(--primary))" }} />
              </div>
              <p className="text-[14px] font-semibold text-foreground mb-1">No words yet</p>
              <p className="text-[12px] text-muted-foreground max-w-xs leading-relaxed">
                Add a name AirNote gets wrong, or just fix it after dictating.
              </p>
            </div>
          </div>
        ) : (
          <>
            {words.length > 8 && (
              <div className="field h-8 mb-4">
                <Search size={13} className="text-muted-foreground flex-shrink-0" />
                <input value={search} onChange={(e) => setSearch(e.target.value)} placeholder="Search words…" className="text-[12.5px]" />
              </div>
            )}
            <div className="vocab-list">
              <div className="vocab-list-head flex items-center gap-3 pl-4 pr-2 h-8 text-[11px] font-medium text-muted-foreground">
                <span className="flex-1">Heard → Written</span>
                <span className="w-20 text-right">From</span>
                <span className="w-20 text-right">Added</span>
                <span className="w-7" />
              </div>
              {shown.map((w) => <WordRow key={w.id} word={w} onDelete={handleDelete} />)}
              {shown.length === 0 && (
                <p className="text-[12px] text-muted-foreground px-4 py-6 text-center">Nothing matches “{search}”.</p>
              )}
            </div>
          </>
        )}
      </div>
    </ScrollArea>
  );
}
