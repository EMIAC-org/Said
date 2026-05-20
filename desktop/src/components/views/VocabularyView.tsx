import React, { useEffect, useRef, useState } from "react";
import {
  BookOpen,
  Sparkles,
  Star,
  Trash2,
  Plus,
  Search,
  X,
  AlertCircle,
} from "lucide-react";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  listVocabulary,
  addVocabularyTerm,
  deleteVocabularyTerm,
  starVocabularyTerm,
  patchVocabularyTerm,
  onVocabularyChanged,
  requestNotifications,
  type VocabRow,
} from "@/lib/invoke";

// ── Source-icon helper ───────────────────────────────────────────────────────

function SourceBadge({ source }: { source: VocabRow["source"] }) {
  if (source === "starred") {
    return (
      <span
        className="inline-flex items-center gap-1 text-[10px] font-bold px-1.5 py-0.5 rounded"
        style={{
          color:      "hsl(var(--chip-amber-fg))",
          background: "hsl(var(--chip-amber-bg))",
        }}
        title="Said keeps starred words even if you stop using them"
      >
        <Star size={9} fill="currentColor" />
        Starred
      </span>
    );
  }
  if (source === "manual") {
    return (
      <span
        className="inline-flex items-center gap-1 text-[10px] font-bold px-1.5 py-0.5 rounded"
        style={{
          color:      "hsl(var(--chip-blue-fg))",
          background: "hsl(var(--chip-blue-bg))",
        }}
        title="Added manually"
      >
        Manual
      </span>
    );
  }
  // auto
  return (
    <span
      className="inline-flex items-center gap-1 text-[10px] font-bold px-1.5 py-0.5 rounded"
      style={{
        color:      "hsl(var(--chip-mint-fg))",
        background: "hsl(var(--chip-mint-bg))",
      }}
      title="Learned automatically from your edits"
    >
      <Sparkles size={9} />
      Auto
    </span>
  );
}

// ── Single vocabulary row ────────────────────────────────────────────────────

interface RowProps {
  row:       VocabRow;
  onStar:    (term: string) => void;
  onDelete:  (term: string) => void;
  onClick:   (row: VocabRow) => void;
}

function VocabRowItem({ row, onStar, onDelete, onClick }: RowProps) {
  const isStarred = row.source === "starred";

  return (
    <div
      className="relative flex items-center gap-4 px-5 py-4 transition-colors group cursor-pointer"
      onClick={() => onClick(row)}
      onMouseEnter={(e) => { e.currentTarget.style.background = "hsl(var(--surface-hover))"; }}
      onMouseLeave={(e) => { e.currentTarget.style.background = "transparent"; }}
    >
      {/* Term */}
      <div className="flex-1 min-w-0">
        <div className="flex items-center gap-2 flex-wrap">
          <span className="text-[14px] text-foreground font-medium tabular-nums">
            {row.term}
          </span>
          <SourceBadge source={row.source} />
          {row.term_type && row.term_type !== "other" && (
            <span className="text-[10px] px-1.5 py-0.5 rounded"
              style={{ background: "hsl(var(--surface-4))", color: "hsl(var(--muted-foreground))" }}>
              {row.term_type.replace("_", " ")}
            </span>
          )}
        </div>
        {row.meaning && (
          <p className="text-[11px] text-muted-foreground mt-0.5 truncate max-w-[300px]">
            {row.meaning}
          </p>
        )}
        <div className="flex items-center gap-3 mt-1">
          <span className="text-[11px] text-muted-foreground tabular-nums">
            used {row.use_count}×
          </span>
          <span className="text-[11px] text-muted-foreground tabular-nums">
            weight {row.weight.toFixed(2)}
          </span>
        </div>
      </div>

      {/* Action buttons — visible on hover (or always for starred) */}
      <div
        className={`flex-shrink-0 flex items-center gap-1 transition-opacity ${
          isStarred ? "opacity-100" : "opacity-0 group-hover:opacity-100"
        }`}
      >
        <button
          onClick={() => onStar(row.term)}
          title={isStarred ? "Unstar this word" : "Star to keep this word permanently"}
          className="w-7 h-7 rounded-lg flex items-center justify-center transition-colors"
          style={{
            color: isStarred ? "hsl(var(--chip-amber-fg))" : "hsl(var(--muted-foreground))",
          }}
          onMouseEnter={(e) => { e.currentTarget.style.background = "hsl(var(--surface-4))"; }}
          onMouseLeave={(e) => { e.currentTarget.style.background = "transparent"; }}
        >
          <Star size={13} fill={isStarred ? "currentColor" : "none"} />
        </button>

        <button
          onClick={() => onDelete(row.term)}
          title="Delete from vocabulary"
          className="w-7 h-7 rounded-lg flex items-center justify-center transition-colors"
          style={{ color: "hsl(var(--muted-foreground))" }}
          onMouseEnter={(e) => {
            e.currentTarget.style.background = "hsl(var(--surface-4))";
            e.currentTarget.style.color      = "hsl(0 75% 62%)";
          }}
          onMouseLeave={(e) => {
            e.currentTarget.style.background = "transparent";
            e.currentTarget.style.color      = "hsl(var(--muted-foreground))";
          }}
        >
          <Trash2 size={13} />
        </button>
      </div>
    </div>
  );
}

// ── Vocab Detail Modal ──────────────────────────────────────────────────────

function VocabDetailModal({
  row,
  onClose,
  onSave,
}: {
  row: VocabRow;
  onClose: () => void;
  onSave: () => void;
}) {
  const [meaning, setMeaning] = useState(row.meaning ?? "");
  const [termType, setTermType] = useState(row.term_type ?? "other");
  const [exampleCtx, setExampleCtx] = useState(row.example_context ?? "");
  const [saving, setSaving] = useState(false);

  const TERM_TYPES = ["proper_noun", "brand", "acronym", "code_identifier", "phrase", "other"];

  async function handleSave() {
    setSaving(true);
    try {
      await patchVocabularyTerm(row.term, {
        meaning: meaning.trim(),
        term_type: termType,
        example_context: exampleCtx.trim(),
      });
      onSave();
      onClose();
    } finally {
      setSaving(false);
    }
  }

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center"
      style={{ background: "rgba(0,0,0,0.6)", backdropFilter: "blur(4px)" }}
      onClick={onClose}
    >
      <div
        className="w-[420px] rounded-xl overflow-hidden"
        style={{ background: "hsl(var(--surface-1))", border: "1px solid hsl(var(--surface-3))" }}
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div className="px-5 pt-5 pb-3 flex items-center justify-between">
          <div>
            <h3 className="text-[16px] font-semibold text-foreground">{row.term}</h3>
            <p className="text-[11px] text-muted-foreground mt-0.5">
              used {row.use_count}x · weight {row.weight.toFixed(2)} · {row.source}
            </p>
          </div>
          <button
            onClick={onClose}
            className="w-7 h-7 rounded-lg flex items-center justify-center"
            style={{ color: "hsl(var(--muted-foreground))" }}
          >
            <X size={14} />
          </button>
        </div>

        {/* Fields */}
        <div className="px-5 pb-4 space-y-3">
          {/* Term Type */}
          <div>
            <label className="text-[11px] text-muted-foreground font-medium block mb-1">Type</label>
            <select
              value={termType}
              onChange={(e) => setTermType(e.target.value)}
              className="w-full px-3 py-1.5 rounded-lg text-[13px] text-foreground"
              style={{ background: "hsl(var(--surface-3))", border: "1px solid hsl(var(--surface-4))" }}
            >
              {TERM_TYPES.map((t) => (
                <option key={t} value={t}>{t.replace("_", " ")}</option>
              ))}
            </select>
          </div>

          {/* Meaning */}
          <div>
            <label className="text-[11px] text-muted-foreground font-medium block mb-1">Meaning</label>
            <textarea
              value={meaning}
              onChange={(e) => setMeaning(e.target.value)}
              placeholder="What this term means (1-2 sentences)"
              rows={2}
              className="w-full px-3 py-2 rounded-lg text-[13px] text-foreground resize-none"
              style={{ background: "hsl(var(--surface-3))", border: "1px solid hsl(var(--surface-4))" }}
            />
          </div>

          {/* Example Context */}
          <div>
            <label className="text-[11px] text-muted-foreground font-medium block mb-1">Example context</label>
            <textarea
              value={exampleCtx}
              onChange={(e) => setExampleCtx(e.target.value)}
              placeholder="A sentence where this term is used"
              rows={2}
              className="w-full px-3 py-2 rounded-lg text-[13px] text-foreground resize-none"
              style={{ background: "hsl(var(--surface-3))", border: "1px solid hsl(var(--surface-4))" }}
            />
          </div>
        </div>

        {/* Footer */}
        <div className="px-5 pb-5 flex justify-end gap-2">
          <button
            onClick={onClose}
            className="px-4 py-1.5 rounded-lg text-[12px] font-medium text-muted-foreground"
            style={{ border: "1px solid hsl(var(--surface-4))" }}
          >
            Cancel
          </button>
          <button
            onClick={() => void handleSave()}
            disabled={saving}
            className="px-4 py-1.5 rounded-lg text-[12px] font-medium text-white"
            style={{ background: "hsl(var(--accent-violet))", opacity: saving ? 0.6 : 1 }}
          >
            {saving ? "Saving..." : "Save"}
          </button>
        </div>
      </div>
    </div>
  );
}

// ── Add-term input row ───────────────────────────────────────────────────────

function AddTermRow({ onAdd }: { onAdd: (term: string) => Promise<void> }) {
  const [value, setValue] = useState("");
  const [busy,  setBusy]  = useState(false);
  const [error, setError] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const term = value.trim();
    if (term.length === 0) return;
    if (term.length > 64) {
      setError("Max 64 characters");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await onAdd(term);
      setValue("");
      inputRef.current?.focus();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to add term");
    } finally {
      setBusy(false);
    }
  }

  return (
    <form
      onSubmit={handleSubmit}
      className="tile flex items-center gap-2 px-3 py-2.5 mb-5"
    >
      <Plus size={14} className="text-muted-foreground flex-shrink-0 ml-1" />
      <input
        ref={inputRef}
        type="text"
        value={value}
        onChange={(e) => { setValue(e.target.value); setError(null); }}
        placeholder="Add a name or word Said often gets wrong, like n8n or Vipassana"
        maxLength={64}
        disabled={busy}
        className="flex-1 bg-transparent outline-none text-[13.5px] text-foreground placeholder:text-muted-foreground/70"
      />
      {error && (
        <span
          className="text-[11px] flex items-center gap-1"
          style={{ color: "hsl(0 75% 62%)" }}
        >
          <AlertCircle size={11} />
          {error}
        </span>
      )}
      <button
        type="submit"
        disabled={busy || value.trim().length === 0}
        className="btn-primary !py-1.5 !px-3 !text-[12px] disabled:opacity-50"
      >
        {busy ? "Adding…" : "Add"}
      </button>
    </form>
  );
}

// ── Search bar ───────────────────────────────────────────────────────────────

function SearchBar({
  value,
  onChange,
}: {
  value: string;
  onChange: (v: string) => void;
}) {
  return (
    <div
      className="flex items-center gap-2 px-3 py-2 mb-4 rounded-xl"
      style={{
        background:  "hsl(var(--surface-4))",
        boxShadow:   "inset 0 0 0 1px hsl(var(--border))",
      }}
    >
      <Search size={13} className="text-muted-foreground flex-shrink-0" />
      <input
        type="text"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder="Filter terms…"
        className="flex-1 bg-transparent outline-none text-[12.5px] text-foreground placeholder:text-muted-foreground/70"
      />
      {value.length > 0 && (
        <button
          onClick={() => onChange("")}
          className="text-muted-foreground hover:text-foreground transition-colors"
          title="Clear filter"
        >
          <X size={12} />
        </button>
      )}
    </div>
  );
}

// ── Main view ────────────────────────────────────────────────────────────────

export function VocabularyView() {
  const [rows,    setRows]    = useState<VocabRow[]>([]);
  const [filter,  setFilter]  = useState("");
  const [loading, setLoading] = useState(true);
  const [detailRow, setDetailRow] = useState<VocabRow | null>(null);

  // Note: confirmation toasts (add / star / delete) are emitted by the
  // backend as `vocab-toast` events and handled by the global toast in
  // App.tsx — no inline toast state needed here.

  async function refresh() {
    const resp = await listVocabulary();
    setRows(resp.terms);
    setLoading(false);
  }

  useEffect(() => {
    refresh();
    const unsub = onVocabularyChanged(refresh);
    // Quietly request macOS notification permission on first mount.
    requestNotifications().catch(() => {});
    return () => unsub();
  }, []);

  async function handleAdd(term: string) {
    await addVocabularyTerm(term);
    await refresh();
  }

  async function handleStar(term: string) {
    await starVocabularyTerm(term);
    await refresh();
  }

  async function handleDelete(term: string) {
    await deleteVocabularyTerm(term);
    setRows((prev) => prev.filter((r) => r.term !== term));
  }

  // Apply filter (case-insensitive substring on term).
  const filtered = rows.filter((r) =>
    r.term.toLowerCase().includes(filter.trim().toLowerCase()),
  );

  // Group: starred first, then auto + manual sorted by weight desc.
  const starred  = filtered.filter((r) => r.source === "starred");
  const learned  = filtered.filter((r) => r.source !== "starred");

  const empty = !loading && rows.length === 0;

  return (
    <ScrollArea className="h-full">
      <div className="p-7 pb-12 max-w-3xl mx-auto">
        {/* ── Header ─────────────────────────────────────────── */}
        <div className="mb-7">
          <h1 className="text-[28px] font-bold tracking-tight text-foreground leading-tight">
            Vocabulary
          </h1>
          <p className="text-[13px] text-muted-foreground mt-1 tabular-nums">
            {rows.length} word{rows.length !== 1 ? "s" : ""} Said remembers when you dictate
          </p>
        </div>

        {/* ── Add term ──────────────────────────────────────── */}
        <AddTermRow onAdd={handleAdd} />

        {/* ── Empty state ───────────────────────────────────── */}
        {empty ? (
          <div className="flex items-center justify-center py-16">
            <div className="text-center px-8">
              <div
                className="w-12 h-12 rounded-full flex items-center justify-center mx-auto mb-4"
                style={{ background: "hsl(var(--primary) / 0.15)" }}
              >
                <BookOpen size={20} style={{ color: "hsl(var(--chip-lime-fg))" }} />
              </div>
              <p className="text-[14px] font-semibold text-foreground mb-1">
                No vocabulary yet
              </p>
              <p className="text-[12px] text-muted-foreground max-w-xs leading-relaxed">
                Names and words you add or correct land here automatically.
                Said will use them on your next recording.
              </p>
            </div>
          </div>
        ) : (
          <>
            {/* ── Search ─────────────────────────────────────── */}
            {rows.length > 5 && (
              <SearchBar value={filter} onChange={setFilter} />
            )}

            {/* ── Starred section ────────────────────────────── */}
            {starred.length > 0 && (
              <div className="mb-7">
                <div className="flex items-center justify-between mb-3 px-1">
                  <span className="section-label">Starred</span>
                  <span className="text-[10px] text-muted-foreground tabular-nums">
                    {starred.length} pinned
                  </span>
                </div>
                <div className="tile overflow-hidden">
                  {starred.map((row, idx) => (
                    <React.Fragment key={row.term}>
                      {idx > 0 && (
                        <div
                          className="mx-5 border-t"
                          style={{ borderColor: "hsl(var(--surface-3))" }}
                        />
                      )}
                      <VocabRowItem
                        row={row}
                        onStar={handleStar}
                        onDelete={handleDelete}
                        onClick={setDetailRow}
                      />
                    </React.Fragment>
                  ))}
                </div>
              </div>
            )}

            {/* ── Learned section ────────────────────────────── */}
            {learned.length > 0 && (
              <div>
                <div className="flex items-center justify-between mb-3 px-1">
                  <span className="section-label">Learned</span>
                  <span className="text-[10px] text-muted-foreground tabular-nums">
                    {learned.length} term{learned.length !== 1 ? "s" : ""}
                  </span>
                </div>
                <div className="tile overflow-hidden">
                  {learned.map((row, idx) => (
                    <React.Fragment key={row.term}>
                      {idx > 0 && (
                        <div
                          className="mx-5 border-t"
                          style={{ borderColor: "hsl(var(--surface-3))" }}
                        />
                      )}
                      <VocabRowItem
                        row={row}
                        onStar={handleStar}
                        onDelete={handleDelete}
                        onClick={setDetailRow}
                      />
                    </React.Fragment>
                  ))}
                </div>
              </div>
            )}

            {/* ── Filter empty state ─────────────────────────── */}
            {filter.trim().length > 0 && filtered.length === 0 && (
              <div className="text-center py-10">
                <p className="text-[12px] text-muted-foreground">
                  No terms match “{filter}”
                </p>
              </div>
            )}
          </>
        )}
      </div>

      {detailRow && (
        <VocabDetailModal
          row={detailRow}
          onClose={() => setDetailRow(null)}
          onSave={refresh}
        />
      )}
    </ScrollArea>
  );
}
