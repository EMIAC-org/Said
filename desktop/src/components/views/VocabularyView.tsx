import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  BookOpen, Sparkles, Star, Trash2, Search, X, Plus, Check, AlertTriangle,
  Pencil, ChevronDown, Undo2, RotateCw, Wand2, ArrowRight,
} from "lucide-react";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Skeleton } from "@/components/ui/skeleton";
import {
  addVocabularyTerm,
  deleteVocabularyTerm,
  resetAllVocabulary,
  starVocabularyTerm,
  patchVocabularyTerm,
  requestNotifications,
  type VocabRow,
  type VocabAlias,
} from "@/lib/invoke";
import { friendlyError } from "@/lib/friendlyError";
import {
  getVocabularyCacheSnapshot,
  invalidateVocabularyCache,
  refreshVocabularyCache,
  subscribeVocabularyCache,
} from "@/lib/vocabularyUiCache";

// Backend caps a manual term at 64 chars (routes/vocabulary.rs create()).
const MAX_TERM_LEN = 64;

/**
 * The `source` column can hold values the UI doesn't model explicitly
 * (e.g. "confirmed" from the edit-confirm flow). Fold anything that isn't a
 * first-class source into "auto" so filters and the row anchor stay coherent.
 */
type KnownSource = "auto" | "manual" | "starred";
function normalizeSource(source: string): KnownSource {
  return source === "starred" || source === "manual" ? source : "auto";
}

// ── Formatting helpers ────────────────────────────────────────────────────────

const TYPE_LABEL: Record<string, string> = {
  proper_noun:     "Name",
  brand:           "Brand",
  acronym:         "Acronym",
  code_identifier: "Code",
  phrase:          "Phrase",
  other:           "Other",
};
function typeLabel(t: string | null | undefined): string {
  if (!t || t === "other") return "";
  return TYPE_LABEL[t] ?? t.replace(/_/g, " ");
}

/** last_used is now_ms() from the backend — same clock as Date.now(). */
function relativeTime(ms: number | null | undefined): string {
  if (!ms || ms <= 0) return "";
  const diff = Date.now() - ms;
  if (diff < 45_000) return "just now";
  const m = Math.floor(diff / 60_000);
  if (m < 60) return `${m}m ago`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ago`;
  const d = Math.floor(h / 24);
  if (d === 1) return "yesterday";
  if (d < 7) return `${d}d ago`;
  if (d < 30) return `${Math.floor(d / 7)}w ago`;
  return new Date(ms).toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

// ── The honest status model ───────────────────────────────────────────────────
// A term's real runtime effect, in priority order:
//   correcting — has ≥1 active learned alias → rewrites mishearings post-LLM
//   starred    — pinned as an important local speech/polish hint
//   glossary   — has a meaning → fed to the polish LLM as a soft hint
//   idle       — known term, but nothing is correcting or biasing yet

type StatusKey = "correcting" | "starred" | "glossary" | "idle";

interface TermInfo {
  row: VocabRow;
  fixes: VocabAlias[];        // learned mishearings for this term
  activeFixes: number;        // how many actually fire at runtime
  status: StatusKey;
}

const STATUS_META: Record<StatusKey, { label: string; color: string; bg: string; blurb: string }> = {
  correcting: { label: "Correcting", color: "hsl(var(--chip-lime-fg))", bg: "hsl(var(--chip-lime-bg))", blurb: "Auto-fixes STT mishearings in your dictation" },
  starred:    { label: "Pinned hint",  color: "hsl(var(--chip-amber-fg))", bg: "hsl(var(--chip-amber-bg))", blurb: "Kept prominent for local speech and polish hints" },
  glossary:   { label: "Glossary hint", color: "hsl(var(--chip-blue-fg))", bg: "hsl(var(--chip-blue-bg))", blurb: "Given to the polish model as a soft hint" },
  idle:       { label: "Idle", color: "hsl(var(--muted-foreground))", bg: "hsl(var(--surface-4))", blurb: "Known, but not correcting or biasing yet — correct it once in use to teach a fix" },
};

function deriveStatus(row: VocabRow, activeFixes: number): StatusKey {
  if (activeFixes > 0) return "correcting";
  if (row.source === "starred") return "starred";
  if ((row.meaning ?? "").trim().length > 0) return "glossary";
  return "idle";
}

/** A term "does something" at runtime if it's correcting or biasing STT. */
function isActive(info: TermInfo): boolean {
  return info.status === "correcting" || info.status === "starred";
}

// ── Toasts (mirrors HistoryView for a consistent surface) ─────────────────────

type ToastKind = "success" | "error" | "info";
interface Toast {
  id: number;
  kind: ToastKind;
  title: string;
  sub?: string;
  action?: { label: string; onClick: () => void };
  duration: number; // ms; 0 = sticky
}

let _toastSeq = 1;

function useToasts() {
  const [toasts, setToasts] = useState<Toast[]>([]);
  const timers = useRef(new Map<number, ReturnType<typeof setTimeout>>());

  const dismiss = useCallback((id: number) => {
    setToasts((prev) => prev.filter((t) => t.id !== id));
    const timer = timers.current.get(id);
    if (timer) { clearTimeout(timer); timers.current.delete(id); }
  }, []);

  const push = useCallback((t: Omit<Toast, "id">): number => {
    const id = _toastSeq++;
    setToasts((prev) => [...prev.slice(-3), { ...t, id }]);
    if (t.duration > 0) {
      timers.current.set(id, setTimeout(() => dismiss(id), t.duration));
    }
    return id;
  }, [dismiss]);

  useEffect(() => () => { timers.current.forEach(clearTimeout); }, []);
  return { toasts, push, dismiss };
}

const TOAST_ICON: Record<ToastKind, React.ReactNode> = {
  success: <Check size={13} strokeWidth={2.5} />,
  error: <AlertTriangle size={13} strokeWidth={2.4} />,
  info: <Sparkles size={12} />,
};
const TOAST_TINT: Record<ToastKind, { bg: string; fg: string }> = {
  success: { bg: "hsl(var(--chip-lime-bg))", fg: "hsl(var(--chip-lime-fg))" },
  error: { bg: "hsl(2 70% 60% / 0.16)", fg: "hsl(2 78% 66%)" },
  info: { bg: "hsl(var(--primary) / 0.16)", fg: "hsl(var(--primary))" },
};

function Toaster({ toasts, onDismiss }: { toasts: Toast[]; onDismiss: (id: number) => void }) {
  if (toasts.length === 0) return null;
  return (
    <div className="fixed bottom-5 left-1/2 -translate-x-1/2 z-50 flex flex-col items-center gap-2 pointer-events-none">
      {toasts.map((t) => {
        const tint = TOAST_TINT[t.kind];
        return (
          <div
            key={t.id}
            className="pointer-events-auto flex items-center gap-3 px-4 py-2.5 rounded-2xl max-w-md w-max"
            style={{
              background: "hsl(var(--surface-3))",
              border: "1px solid hsl(var(--border))",
              boxShadow: "0 8px 32px hsl(0 0% 0% / 0.28)",
              animation: "fadeIn 0.18s ease-out",
            }}
          >
            <span className="w-7 h-7 rounded-full flex items-center justify-center flex-shrink-0" style={{ background: tint.bg, color: tint.fg }}>
              {TOAST_ICON[t.kind]}
            </span>
            <div className="flex-1 min-w-0">
              <p className="text-[12px] font-semibold text-foreground leading-tight">{t.title}</p>
              {t.sub && <p className="text-[11px] text-muted-foreground leading-tight mt-0.5 truncate" title={t.sub}>{t.sub}</p>}
            </div>
            {t.action && (
              <button
                onClick={() => { t.action!.onClick(); onDismiss(t.id); }}
                className="flex items-center gap-1 px-2.5 py-1 rounded-lg text-[11px] font-semibold transition-colors flex-shrink-0"
                style={{ color: "hsl(var(--primary))", background: "hsl(var(--primary) / 0.12)" }}
              >
                <Undo2 size={11} /> {t.action.label}
              </button>
            )}
            <button onClick={() => onDismiss(t.id)} title="Dismiss" className="text-muted-foreground hover:text-foreground transition-colors flex-shrink-0">
              <X size={13} />
            </button>
          </div>
        );
      })}
    </div>
  );
}

// ── Source anchor (colored icon box) ──────────────────────────────────────────

const SOURCE_ANCHOR: Record<KnownSource, { icon: React.ReactNode; bg: string; fg: string; title: string }> = {
  starred: { icon: <Star size={15} fill="currentColor" />, bg: "hsl(var(--chip-amber-bg))", fg: "hsl(var(--chip-amber-fg))", title: "You pinned this word" },
  manual:  { icon: <Pencil size={14} />,                    bg: "hsl(var(--chip-blue-bg))",  fg: "hsl(var(--chip-blue-fg))",  title: "You added this word yourself" },
  auto:    { icon: <Sparkles size={14} />,                  bg: "hsl(var(--chip-mint-bg))",  fg: "hsl(var(--chip-mint-fg))",  title: "Learned automatically from your corrections" },
};

// ── Single term card ──────────────────────────────────────────────────────────

interface RowProps {
  info: TermInfo;
  flash: boolean;
  expanded: boolean;
  onToggleExpand: (term: string) => void;
  onStar: (row: VocabRow) => void;
  onDelete: (row: VocabRow) => void;
  onEdit: (row: VocabRow) => void;
}

function VocabRowItem({ info, flash, expanded, onToggleExpand, onStar, onDelete, onEdit }: RowProps) {
  const { row, fixes, activeFixes, status } = info;
  const isStarred = row.source === "starred";
  const a = SOURCE_ANCHOR[normalizeSource(row.source)];
  const st = STATUS_META[status];
  const hasFixes = fixes.length > 0;

  return (
    <div
      className="vocab-card group relative rounded-xl transition-colors"
      style={flash
        ? { boxShadow: "inset 0 0 0 1px hsl(var(--primary) / 0.5)", background: "hsl(var(--primary) / 0.10)" }
        : { boxShadow: "inset 0 0 0 1px hsl(var(--border))", background: "hsl(var(--surface-1))" }}
      onMouseEnter={(e) => { if (!flash) e.currentTarget.style.background = "hsl(var(--surface-2))"; }}
      onMouseLeave={(e) => { if (!flash) e.currentTarget.style.background = "hsl(var(--surface-1))"; }}
    >
      <div className="flex items-center gap-3 px-4 py-3 cursor-pointer" onClick={() => onEdit(row)}>
        {/* Source anchor */}
        <div className="w-8 h-8 flex-shrink-0 rounded-lg flex items-center justify-center" style={{ background: a.bg, color: a.fg }} title={a.title}>
          {a.icon}
        </div>

        {/* Term + status */}
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2 flex-wrap">
            <span className="text-[14px] text-foreground font-medium">{row.term}</span>
            {typeLabel(row.term_type) && (
              <span className="text-[10px] px-1.5 py-0.5 rounded" style={{ background: "hsl(var(--surface-4))", color: "hsl(var(--muted-foreground))" }}>
                {typeLabel(row.term_type)}
              </span>
            )}
            {isStarred && status !== "starred" && (
              <Star size={11} fill="currentColor" style={{ color: "hsl(var(--chip-amber-fg))" }} />
            )}
          </div>
          <div className="flex items-center gap-2 mt-1 text-[11px] flex-wrap">
            <span className="inline-flex items-center gap-1 px-1.5 py-0.5 rounded font-semibold" style={{ background: st.bg, color: st.color }} title={st.blurb}>
              <span className="w-1.5 h-1.5 rounded-full" style={{ background: st.color }} />
              {st.label}
            </span>
            {hasFixes && (
              <button
                onClick={(e) => { e.stopPropagation(); onToggleExpand(row.term); }}
                className="inline-flex items-center gap-1 text-muted-foreground hover:text-foreground transition-colors font-medium"
              >
                {activeFixes > 0 ? `${activeFixes} fix${activeFixes !== 1 ? "es" : ""}` : `${fixes.length} pending`}
                <ChevronDown size={11} style={{ transform: expanded ? "rotate(180deg)" : "none", transition: "transform .15s" }} />
              </button>
            )}
            {row.use_count > 0 && <><span className="text-muted-foreground opacity-40">·</span><span className="text-muted-foreground tabular-nums">used {row.use_count}×</span></>}
            {relativeTime(row.last_used) && <><span className="text-muted-foreground opacity-40">·</span><span className="text-muted-foreground">{relativeTime(row.last_used)}</span></>}
          </div>
        </div>

        {/* Hover actions */}
        <div className={`flex-shrink-0 flex items-center gap-0.5 transition-opacity ${isStarred ? "opacity-100" : "opacity-0 group-hover:opacity-100"}`}>
          <button
            onClick={(e) => { e.stopPropagation(); onStar(row); }}
            title={isStarred ? "Unpin — stop biasing cloud STT" : "Pin — bias cloud STT toward this word"}
            className="w-7 h-7 rounded-lg flex items-center justify-center transition-colors"
            style={{ color: isStarred ? "hsl(var(--chip-amber-fg))" : "hsl(var(--muted-foreground))" }}
            onMouseEnter={(e) => { e.currentTarget.style.background = "hsl(var(--surface-4))"; }}
            onMouseLeave={(e) => { e.currentTarget.style.background = "transparent"; }}
          >
            <Star size={13} fill={isStarred ? "currentColor" : "none"} />
          </button>
          <button
            onClick={(e) => { e.stopPropagation(); onEdit(row); }}
            title="Edit details"
            className="w-7 h-7 rounded-lg flex items-center justify-center transition-colors"
            style={{ color: "hsl(var(--muted-foreground))" }}
            onMouseEnter={(e) => { e.currentTarget.style.background = "hsl(var(--surface-4))"; }}
            onMouseLeave={(e) => { e.currentTarget.style.background = "transparent"; }}
          >
            <Pencil size={12.5} />
          </button>
          <button
            onClick={(e) => { e.stopPropagation(); onDelete(row); }}
            title="Delete term (and its learned fixes)"
            className="w-7 h-7 rounded-lg flex items-center justify-center transition-colors"
            style={{ color: "hsl(var(--muted-foreground))" }}
            onMouseEnter={(e) => { e.currentTarget.style.background = "hsl(var(--surface-4))"; e.currentTarget.style.color = "hsl(0 75% 62%)"; }}
            onMouseLeave={(e) => { e.currentTarget.style.background = "transparent"; e.currentTarget.style.color = "hsl(var(--muted-foreground))"; }}
          >
            <Trash2 size={13} />
          </button>
        </div>
      </div>

      {/* Expanded: the real learned corrections */}
      {expanded && hasFixes && (
        <div className="px-4 pb-3 pt-0.5">
          <div className="rounded-lg overflow-hidden" style={{ background: "hsl(var(--surface-4) / 0.5)", boxShadow: "inset 0 0 0 1px hsl(var(--border))" }}>
            <p className="text-[10px] font-medium uppercase tracking-wide text-muted-foreground px-3 pt-2 pb-1">
              Learned fixes — STT mishearing → corrected
            </p>
            {fixes.map((f, i) => (
              <div key={f.transcript_form + i} className="flex items-center gap-2 px-3 py-1.5 text-[12px]" style={i > 0 ? { borderTop: "1px solid hsl(var(--border))" } : undefined}>
                <span className="font-mono text-muted-foreground line-through decoration-[hsl(2_70%_60%)]/50">{f.transcript_form}</span>
                <ArrowRight size={11} className="text-muted-foreground flex-shrink-0" />
                <span className="font-mono text-foreground font-medium">{f.correct_form}</span>
                <span className="flex-1" />
                {f.use_count > 0 && <span className="text-[10px] text-muted-foreground tabular-nums">{f.use_count}×</span>}
                {!f.active && (
                  <span className="text-[9px] px-1.5 py-0.5 rounded font-semibold" style={{ background: "hsl(var(--surface-3))", color: "hsl(var(--muted-foreground))" }} title="Not yet approved — won't fire until confirmed">
                    pending
                  </span>
                )}
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

// ── Section wrapper ───────────────────────────────────────────────────────────

function Section({ label, count, hint, children }: { label: string; count: number; hint?: string; children: React.ReactNode }) {
  return (
    <div className="mb-6">
      <div className="flex items-center justify-between mb-2.5 px-1">
        <span className="section-label">{label}{hint && <span className="ml-2 font-normal normal-case tracking-normal text-muted-foreground/70">{hint}</span>}</span>
        <span className="text-[10px] text-muted-foreground tabular-nums">{count}</span>
      </div>
      <div className="space-y-2">{children}</div>
    </div>
  );
}

// ── Detail / edit modal ───────────────────────────────────────────────────────

const TERM_TYPES = ["proper_noun", "brand", "acronym", "code_identifier", "phrase", "other"];

function VocabDetailModal({
  info, onClose, onSaved, onError, onStar, onDelete,
}: {
  info: TermInfo;
  onClose: () => void;
  onSaved: () => void;
  onError: (message: string) => void;
  onStar: (row: VocabRow) => void;
  onDelete: (row: VocabRow) => void;
}) {
  const { row, fixes, activeFixes, status } = info;
  const [meaning, setMeaning] = useState(row.meaning ?? "");
  const [termType, setTermType] = useState(row.term_type ?? "other");
  const [exampleCtx, setExampleCtx] = useState(row.example_context ?? "");
  const [saving, setSaving] = useState(false);
  const meaningRef = useRef<HTMLTextAreaElement>(null);
  const st = STATUS_META[status];

  const dirty =
    meaning.trim() !== (row.meaning ?? "").trim() ||
    termType !== (row.term_type ?? "other") ||
    exampleCtx.trim() !== (row.example_context ?? "").trim();

  const handleSave = useCallback(async () => {
    if (!dirty) { onClose(); return; }
    setSaving(true);
    try {
      await patchVocabularyTerm(row.term, { meaning: meaning.trim(), term_type: termType, example_context: exampleCtx.trim() });
      onSaved();
      onClose();
    } catch (err) {
      onError(friendlyError(err));
      setSaving(false);
    }
  }, [dirty, row.term, meaning, termType, exampleCtx, onSaved, onClose, onError]);

  useEffect(() => {
    meaningRef.current?.focus();
    const h = (e: KeyboardEvent) => {
      if (e.key === "Escape") { e.preventDefault(); onClose(); }
      if ((e.metaKey || e.ctrlKey) && e.key === "Enter") { e.preventDefault(); void handleSave(); }
    };
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, [handleSave, onClose]);

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center" style={{ background: "rgba(0,0,0,0.6)", backdropFilter: "blur(4px)" }} onClick={onClose}>
      <div className="w-[460px] max-h-[86vh] overflow-auto rounded-xl" style={{ background: "hsl(var(--surface-1))", border: "1px solid hsl(var(--surface-3))", boxShadow: "0 20px 60px rgba(0,0,0,0.5)" }} onClick={(e) => e.stopPropagation()}>
        {/* Header */}
        <div className="px-5 pt-5 pb-3 flex items-start justify-between gap-3">
          <div className="min-w-0">
            <h3 className="text-[16px] font-semibold text-foreground truncate">{row.term}</h3>
            <div className="flex items-center gap-2 mt-1.5">
              <span className="inline-flex items-center gap-1 px-1.5 py-0.5 rounded text-[11px] font-semibold" style={{ background: st.bg, color: st.color }}>
                <span className="w-1.5 h-1.5 rounded-full" style={{ background: st.color }} /> {st.label}
              </span>
              {row.use_count > 0 && <span className="text-[11px] text-muted-foreground">used {row.use_count}×</span>}
            </div>
            <p className="text-[11px] text-muted-foreground mt-1.5 leading-snug">{st.blurb}.</p>
          </div>
          <button onClick={onClose} className="w-7 h-7 rounded-lg flex items-center justify-center flex-shrink-0" style={{ color: "hsl(var(--muted-foreground))" }} title="Close (Esc)">
            <X size={14} />
          </button>
        </div>

        {/* Learned fixes (the real artifact) */}
        {fixes.length > 0 && (
          <div className="px-5 pb-3">
            <label className="text-[11px] text-muted-foreground font-medium block mb-1.5">
              Learned fixes {activeFixes > 0 ? `· ${activeFixes} active` : "· pending approval"}
            </label>
            <div className="rounded-lg overflow-hidden" style={{ background: "hsl(var(--surface-3))", boxShadow: "inset 0 0 0 1px hsl(var(--border))" }}>
              {fixes.map((f, i) => (
                <div key={f.transcript_form + i} className="flex items-center gap-2 px-3 py-1.5 text-[12px]" style={i > 0 ? { borderTop: "1px solid hsl(var(--border))" } : undefined}>
                  <span className="font-mono text-muted-foreground line-through">{f.transcript_form}</span>
                  <ArrowRight size={11} className="text-muted-foreground" />
                  <span className="font-mono text-foreground font-medium">{f.correct_form}</span>
                  <span className="flex-1" />
                  {!f.active && <span className="text-[9px] text-muted-foreground">pending</span>}
                </div>
              ))}
            </div>
          </div>
        )}

        {/* Fields (secondary — feed the polish model) */}
        <div className="px-5 pb-4 space-y-3">
          <p className="text-[10px] text-muted-foreground/80 uppercase tracking-wide">Polish-model hints (optional)</p>
          <div>
            <label className="text-[11px] text-muted-foreground font-medium block mb-1">Type</label>
            <select value={termType} onChange={(e) => setTermType(e.target.value)}
              className="w-full px-3 py-1.5 rounded-lg text-[13px] text-foreground"
              style={{ background: "hsl(var(--surface-3))", border: "1px solid hsl(var(--surface-4))" }}>
              {TERM_TYPES.map((t) => <option key={t} value={t}>{typeLabel(t) || "Other"}</option>)}
            </select>
          </div>
          <div>
            <label className="text-[11px] text-muted-foreground font-medium block mb-1">Meaning</label>
            <textarea ref={meaningRef} value={meaning} onChange={(e) => setMeaning(e.target.value)}
              placeholder="What this term means — helps the polish model use it in context" rows={2}
              className="w-full px-3 py-2 rounded-lg text-[13px] text-foreground resize-none"
              style={{ background: "hsl(var(--surface-3))", border: "1px solid hsl(var(--surface-4))" }} />
          </div>
          <div>
            <label className="text-[11px] text-muted-foreground font-medium block mb-1">Example context</label>
            <textarea value={exampleCtx} onChange={(e) => setExampleCtx(e.target.value)}
              placeholder="A sentence where you use this term" rows={2}
              className="w-full px-3 py-2 rounded-lg text-[13px] text-foreground resize-none"
              style={{ background: "hsl(var(--surface-3))", border: "1px solid hsl(var(--surface-4))" }} />
          </div>
        </div>

        {/* Footer */}
        <div className="px-5 pb-5 flex items-center justify-between gap-2">
          <div className="flex items-center gap-1">
            <button onClick={() => onStar(row)} title={row.source === "starred" ? "Unpin" : "Pin — bias cloud STT"}
              className="w-8 h-8 rounded-lg flex items-center justify-center transition-colors"
              style={{ color: row.source === "starred" ? "hsl(var(--chip-amber-fg))" : "hsl(var(--muted-foreground))", border: "1px solid hsl(var(--surface-4))" }}>
              <Star size={13} fill={row.source === "starred" ? "currentColor" : "none"} />
            </button>
            <button onClick={() => { onClose(); onDelete(row); }} title="Delete term and its learned fixes"
              className="w-8 h-8 rounded-lg flex items-center justify-center transition-colors"
              style={{ color: "hsl(var(--muted-foreground))", border: "1px solid hsl(var(--surface-4))" }}
              onMouseEnter={(e) => { e.currentTarget.style.color = "hsl(0 75% 62%)"; }}
              onMouseLeave={(e) => { e.currentTarget.style.color = "hsl(var(--muted-foreground))"; }}>
              <Trash2 size={13} />
            </button>
          </div>
          <div className="flex items-center gap-2">
            <button onClick={onClose} className="px-4 py-1.5 rounded-lg text-[12px] font-medium text-muted-foreground" style={{ border: "1px solid hsl(var(--surface-4))" }}>Cancel</button>
            <button onClick={() => void handleSave()} disabled={saving || !dirty}
              className="px-4 py-1.5 rounded-lg text-[12px] font-medium text-white transition-opacity"
              style={{ background: "hsl(var(--accent-violet))", opacity: saving || !dirty ? 0.5 : 1 }} title={dirty ? "Save (⌘↵)" : "No changes"}>
              {saving ? "Saving…" : "Save"}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

// ── Sort + status filter ──────────────────────────────────────────────────────

type SortKey = "fixes" | "used" | "recent" | "az";
const SORT_OPTIONS: { value: SortKey; label: string }[] = [
  { value: "fixes", label: "Most fixes" },
  { value: "used", label: "Most used" },
  { value: "recent", label: "Recently used" },
  { value: "az", label: "A–Z" },
];

function SortDropdown({ value, onChange }: { value: SortKey; onChange: (v: SortKey) => void }) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const h = (e: MouseEvent) => { if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false); };
    document.addEventListener("mousedown", h);
    return () => document.removeEventListener("mousedown", h);
  }, [open]);
  const current = SORT_OPTIONS.find((o) => o.value === value)?.label ?? "Sort";
  return (
    <div ref={ref} className="relative">
      <button onClick={() => setOpen((o) => !o)}
        className="flex items-center gap-1.5 px-3 h-8 rounded-lg text-[12px] font-medium transition-colors"
        style={{ background: "hsl(var(--surface-4))", color: "hsl(var(--foreground))", boxShadow: "inset 0 0 0 1px hsl(var(--border))" }}>
        {current}<ChevronDown size={13} className="text-muted-foreground" />
      </button>
      {open && (
        <div className="absolute right-0 top-9 z-40 rounded-xl py-1.5 px-1.5 min-w-[150px]"
          style={{ background: "hsl(var(--surface-1))", border: "1px solid hsl(var(--surface-3))", boxShadow: "0 8px 32px rgba(0,0,0,0.4)" }}>
          {SORT_OPTIONS.map((o) => (
            <button key={o.value} onClick={() => { onChange(o.value); setOpen(false); }}
              className="w-full flex items-center justify-between gap-2 px-2.5 py-1.5 text-left text-[12.5px] rounded-lg transition-colors"
              style={{ color: o.value === value ? "hsl(var(--foreground))" : "hsl(var(--muted-foreground))" }}
              onMouseEnter={(e) => { e.currentTarget.style.background = "hsl(var(--surface-4))"; }}
              onMouseLeave={(e) => { e.currentTarget.style.background = "transparent"; }}>
              {o.label}{o.value === value && <Check size={12} style={{ color: "hsl(var(--primary))" }} />}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

type StatusFilter = "all" | "correcting" | "idle";

function StatusChips({ value, counts, onChange }: {
  value: StatusFilter; counts: Record<StatusFilter, number>; onChange: (v: StatusFilter) => void;
}) {
  const chips: { value: StatusFilter; label: string }[] = [
    { value: "all", label: "All" },
    { value: "correcting", label: "Correcting" },
    { value: "idle", label: "Idle" },
  ];
  return (
    <div className="flex items-center gap-1.5">
      {chips.map((c) => {
        const active = value === c.value;
        if (c.value !== "all" && counts[c.value] === 0) return null;
        return (
          <button key={c.value} onClick={() => onChange(c.value)}
            className="px-2.5 h-8 rounded-lg text-[12px] font-medium transition-colors tabular-nums"
            style={{
              background: active ? "hsl(var(--primary) / 0.14)" : "hsl(var(--surface-4))",
              color: active ? "hsl(var(--primary))" : "hsl(var(--muted-foreground))",
              boxShadow: active ? "inset 0 0 0 1px hsl(var(--primary) / 0.35)" : "inset 0 0 0 1px hsl(var(--border))",
            }}>
            {c.label} <span className="opacity-60">{counts[c.value]}</span>
          </button>
        );
      })}
    </div>
  );
}

// ── Loading skeleton ──────────────────────────────────────────────────────────

function VocabularySkeleton() {
  return (
    <div className="space-y-2">
      {Array.from({ length: 6 }).map((_, i) => (
        <div key={i} className="flex items-center gap-3 rounded-xl px-4 py-3" style={{ boxShadow: "inset 0 0 0 1px hsl(var(--border))", background: "hsl(var(--surface-1))" }}>
          <Skeleton className="w-8 h-8 rounded-lg flex-shrink-0" />
          <div className="flex-1 space-y-2">
            <Skeleton className="h-3" style={{ width: `${55 - i * 6}%` }} />
            <Skeleton className="h-2.5 w-1/4" />
          </div>
        </div>
      ))}
    </div>
  );
}

// ── Main view ─────────────────────────────────────────────────────────────────

export function VocabularyView() {
  const [rows, setRows] = useState<VocabRow[]>(() => getVocabularyCacheSnapshot().terms ?? []);
  const [aliases, setAliases] = useState<VocabAlias[]>(() => getVocabularyCacheSnapshot().aliases ?? []);
  const [loading, setLoading] = useState(() => {
    const cached = getVocabularyCacheSnapshot();
    return cached.terms === undefined || cached.aliases === undefined;
  });
  const [search, setSearch] = useState("");
  const [statusFilter, setStatusFilter] = useState<StatusFilter>("all");
  const [sort, setSort] = useState<SortKey>("fixes");
  const [detailTerm, setDetailTerm] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [addValue, setAddValue] = useState("");
  const [adding, setAdding] = useState(false);
  const [flashTerm, setFlashTerm] = useState<string | null>(null);
  const [showResetConfirm, setShowResetConfirm] = useState(false);
  const [resetting, setResetting] = useState(false);

  const { toasts, push, dismiss } = useToasts();

  const pendingDeletes = useRef(new Map<string, { timer: ReturnType<typeof setTimeout>; commit: () => void }>());
  const flashTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  const refresh = useCallback(async (force = false) => {
    const cached = await refreshVocabularyCache({ force });
    setRows((cached.terms ?? []).filter((row) => !pendingDeletes.current.has(row.term)));
    setAliases(cached.aliases ?? []);
    setLoading(false);
  }, []);

  useEffect(() => {
    const sync = () => {
      const cached = getVocabularyCacheSnapshot();
      if (cached.terms === undefined || cached.aliases === undefined || cached.stale) return;
      setRows(cached.terms.filter((row) => !pendingDeletes.current.has(row.term)));
      setAliases(cached.aliases);
      setLoading(false);
    };
    sync();
    const unsubscribeCache = subscribeVocabularyCache(sync);
    void refresh();
    requestNotifications().catch(() => {});
    return () => {
      unsubscribeCache();
      pendingDeletes.current.forEach(({ timer, commit }) => { clearTimeout(timer); commit(); });
      pendingDeletes.current.clear();
      if (flashTimer.current) clearTimeout(flashTimer.current);
    };
  }, [refresh]);

  // Aliases grouped by canonical term (lowercased).
  const aliasesByTerm = useMemo(() => {
    const m = new Map<string, VocabAlias[]>();
    for (const a of aliases) {
      const key = a.correct_form.toLowerCase();
      (m.get(key) ?? m.set(key, []).get(key)!).push(a);
    }
    // Newest-firing first: active before pending, then by use_count.
    for (const list of m.values()) list.sort((x, y) => Number(y.active) - Number(x.active) || y.use_count - x.use_count);
    return m;
  }, [aliases]);

  // Build the enriched term list once.
  const infos = useMemo<TermInfo[]>(() => rows.map((row) => {
    const fixes = aliasesByTerm.get(row.term.toLowerCase()) ?? [];
    const activeFixes = fixes.filter((f) => f.active).length;
    return { row, fixes, activeFixes, status: deriveStatus(row, activeFixes) };
  }), [rows, aliasesByTerm]);

  const totalActiveFixes = useMemo(() => infos.reduce((n, i) => n + i.activeFixes, 0), [infos]);
  const correctingCount = useMemo(() => infos.filter((i) => i.status === "correcting").length, [infos]);

  const flash = useCallback((term: string) => {
    setFlashTerm(term);
    if (flashTimer.current) clearTimeout(flashTimer.current);
    flashTimer.current = setTimeout(() => setFlashTerm(null), 1600);
  }, []);

  function toggleExpand(term: string) {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(term)) next.delete(term); else next.add(term);
      return next;
    });
  }

  // ── Add ──
  async function handleAdd() {
    const term = addValue.trim();
    if (!term || adding) return;
    if (term.length > MAX_TERM_LEN) {
      push({ kind: "error", title: "That word is too long", sub: `Keep it under ${MAX_TERM_LEN} characters.`, duration: 4000 });
      return;
    }
    const existing = rows.find((r) => r.term.toLowerCase() === term.toLowerCase());
    if (existing) {
      setAddValue(""); setSearch(""); setStatusFilter("all");
      flash(existing.term);
      push({ kind: "info", title: `“${existing.term}” is already in your vocabulary`, duration: 3500 });
      return;
    }
    setAdding(true);
    try {
      await addVocabularyTerm(term);
      invalidateVocabularyCache();
      setAddValue("");
      setRows((prev) => [{ term, weight: 1.5, use_count: 0, last_used: Date.now(), source: "manual", meaning: null, term_type: null, example_context: null }, ...prev.filter((r) => r.term !== term)]);
      flash(term);
      push({
        kind: "success",
        title: `Added “${term}”`,
        sub: "It'll hint the polish model now; it learns to fix mishearings as you correct it.",
        duration: 5000,
        action: { label: "Undo", onClick: () => { setRows((prev) => prev.filter((r) => r.term !== term)); deleteVocabularyTerm(term).catch(() => {}); } },
      });
    } catch (err) {
      push({ kind: "error", title: "Couldn’t add that word", sub: friendlyError(err), duration: 5000 });
    } finally {
      setAdding(false);
    }
  }

  // ── Delete (optimistic + 5s Undo) ──
  function handleDelete(row: VocabRow) {
    const term = row.term;
    if (pendingDeletes.current.has(term)) return;
    setRows((prev) => prev.filter((r) => r.term !== term));
    if (detailTerm === term) setDetailTerm(null);

    const commit = () => {
      pendingDeletes.current.delete(term);
      deleteVocabularyTerm(term)
        .then(() => {
          invalidateVocabularyCache();
          return refresh(true);
        })
        .catch((err) => {
          setRows((prev) => [row, ...prev.filter((r) => r.term !== term)]);
          push({ kind: "error", title: `Couldn’t delete “${term}”`, sub: friendlyError(err), duration: 5000 });
        });
    };
    const timer = setTimeout(commit, 5000);
    pendingDeletes.current.set(term, { timer, commit });

    const fixCount = (aliasesByTerm.get(term.toLowerCase()) ?? []).length;
    push({
      kind: "info",
      title: `Deleted “${term}”`,
      sub: fixCount > 0 ? `Also removes ${fixCount} learned fix${fixCount !== 1 ? "es" : ""}.` : undefined,
      duration: 5000,
      action: {
        label: "Undo",
        onClick: () => {
          const p = pendingDeletes.current.get(term);
          if (p) { clearTimeout(p.timer); pendingDeletes.current.delete(term); }
          setRows((prev) => [row, ...prev.filter((r) => r.term !== term)]);
        },
      },
    });
  }

  // ── Star / unstar ──
  async function handleStar(row: VocabRow) {
    const wasStarred = row.source === "starred";
    setRows((prev) => prev.map((r) => r.term === row.term ? { ...r, source: wasStarred ? "manual" : "starred", weight: wasStarred ? 1.5 : 3.0 } : r));
    try {
      await starVocabularyTerm(row.term);
      invalidateVocabularyCache();
      if (!wasStarred) push({ kind: "success", title: `Pinned “${row.term}”`, sub: "Kept prominent for local speech and polish hints.", duration: 3500 });
    } catch (err) {
      await refresh();
      push({ kind: "error", title: "Couldn’t update that word", sub: friendlyError(err), duration: 5000 });
    }
  }

  // ── Reset ──
  async function handleResetAll() {
    setResetting(true);
    try {
      pendingDeletes.current.forEach(({ timer }) => clearTimeout(timer));
      pendingDeletes.current.clear();
      await resetAllVocabulary();
      invalidateVocabularyCache();
      setRows([]); setAliases([]); setShowResetConfirm(false);
      push({ kind: "success", title: "Learning reset", sub: "Vocabulary, learned fixes and preferences were cleared.", duration: 4500 });
    } catch (err) {
      push({ kind: "error", title: "Couldn’t reset learning", sub: friendlyError(err), duration: 5000 });
    } finally {
      setResetting(false);
    }
  }

  // ── Derived: search + status filter + sort ──
  const q = search.trim().toLowerCase();
  const searched = useMemo(() => infos.filter((i) =>
    !q || i.row.term.toLowerCase().includes(q)
       || (i.row.meaning ?? "").toLowerCase().includes(q)
       || i.fixes.some((f) => f.transcript_form.toLowerCase().includes(q)),
  ), [infos, q]);

  const counts = useMemo<Record<StatusFilter, number>>(() => ({
    all: searched.length,
    correcting: searched.filter((i) => i.status === "correcting").length,
    idle: searched.filter((i) => i.status === "idle").length,
  }), [searched]);

  const sortFn = useCallback((a: TermInfo, b: TermInfo) => {
    if (sort === "az") return a.row.term.localeCompare(b.row.term);
    if (sort === "recent") return b.row.last_used - a.row.last_used;
    if (sort === "used") return b.row.use_count - a.row.use_count || b.activeFixes - a.activeFixes;
    return b.activeFixes - a.activeFixes || b.fixes.length - a.fixes.length || b.row.use_count - a.row.use_count;
  }, [sort]);

  const filtered = useMemo(() => {
    const byStatus = statusFilter === "all" ? searched
      : statusFilter === "correcting" ? searched.filter((i) => i.status === "correcting")
      : searched.filter((i) => i.status === "idle");
    return [...byStatus].sort(sortFn);
  }, [searched, statusFilter, sortFn]);

  const sectioned = statusFilter === "all" && !q;
  const active = useMemo(() => filtered.filter(isActive), [filtered]);
  const inactive = useMemo(() => filtered.filter((i) => !isActive(i)), [filtered]);

  const detailInfo = useMemo(() => infos.find((i) => i.row.term === detailTerm) ?? null, [infos, detailTerm]);
  const empty = !loading && rows.length === 0;
  const noResults = !loading && rows.length > 0 && filtered.length === 0;

  const renderCards = (list: TermInfo[]) => list.map((info) => (
    <VocabRowItem
      key={info.row.term}
      info={info}
      flash={flashTerm === info.row.term}
      expanded={expanded.has(info.row.term)}
      onToggleExpand={toggleExpand}
      onStar={handleStar}
      onDelete={handleDelete}
      onEdit={(r) => setDetailTerm(r.term)}
    />
  ));

  return (
    <ScrollArea className="h-full">
      <div className="p-7 pb-12 max-w-3xl mx-auto">
        {/* ── Header ── */}
        <div className="mb-5 flex items-start justify-between gap-4">
          <div>
            <h1 className="text-[28px] font-bold tracking-tight text-foreground leading-tight">Vocabulary</h1>
            <p className="text-[13px] text-muted-foreground mt-1 tabular-nums">
              {rows.length === 0
                ? "Words AirNote learns to spell right in your dictation"
                : <>{correctingCount} of {rows.length} actively correcting · {totalActiveFixes} learned fix{totalActiveFixes !== 1 ? "es" : ""}</>}
            </p>
          </div>
          {rows.length > 0 && (
            <button onClick={() => setShowResetConfirm(true)}
              className="flex items-center gap-1.5 text-[11px] font-medium px-2.5 py-1.5 rounded-md transition-colors flex-shrink-0"
              style={{ color: "hsl(var(--destructive))", background: "hsl(var(--destructive) / 0.1)", border: "1px solid hsl(var(--destructive) / 0.2)" }}>
              <Trash2 size={12} /> Reset learning
            </button>
          )}
        </div>

        {/* ── Add a word ── */}
        <div className="flex items-center gap-2 mb-2">
          <div className="flex items-center gap-2 flex-1 px-3 h-10 rounded-xl" style={{ background: "hsl(var(--surface-4))", boxShadow: "inset 0 0 0 1px hsl(var(--border))" }}>
            <Plus size={15} className="text-muted-foreground flex-shrink-0" />
            <input
              value={addValue}
              maxLength={MAX_TERM_LEN + 10}
              onChange={(e) => setAddValue(e.target.value)}
              onKeyDown={(e) => { if (e.key === "Enter") void handleAdd(); }}
              placeholder="Add a name, brand or word AirNote keeps getting wrong…"
              className="flex-1 bg-transparent outline-none text-[13px] text-foreground placeholder:text-muted-foreground/70"
            />
            {addValue.length > 0 && (
              <span className={`text-[10px] tabular-nums ${addValue.trim().length > MAX_TERM_LEN ? "text-[hsl(var(--destructive))]" : "text-muted-foreground/60"}`}>
                {addValue.trim().length}/{MAX_TERM_LEN}
              </span>
            )}
          </div>
          <button onClick={() => void handleAdd()} disabled={!addValue.trim() || adding}
            className="px-4 h-10 rounded-xl text-[13px] font-semibold text-white transition-opacity flex-shrink-0"
            style={{ background: "hsl(var(--accent-violet))", opacity: !addValue.trim() || adding ? 0.5 : 1 }}>
            {adding ? "Adding…" : "Add"}
          </button>
        </div>
        <p className="text-[11px] text-muted-foreground/70 mb-5 flex items-center gap-1.5 px-1">
          <Wand2 size={11} />
          Most words land here on their own — AirNote learns a fix each time you correct it while dictating.
        </p>

        {/* ── Reset confirm ── */}
        {showResetConfirm && (
          <div className="mb-5 p-4 rounded-lg border flex items-start gap-3" style={{ background: "hsl(var(--destructive) / 0.06)", borderColor: "hsl(var(--destructive) / 0.25)" }}>
            <AlertTriangle size={18} style={{ color: "hsl(var(--destructive))", flexShrink: 0, marginTop: 2 }} />
            <div className="flex-1">
              <p className="text-[13px] font-semibold text-foreground mb-1">Reset all learning data?</p>
              <p className="text-[12px] text-muted-foreground mb-3">
                This permanently deletes all {rows.length} term{rows.length !== 1 ? "s" : ""}, every learned fix, and learned preferences.
                Your API keys, settings, and recording history are not affected. This can’t be undone.
              </p>
              <div className="flex gap-2">
                <button onClick={() => void handleResetAll()} disabled={resetting}
                  className="text-[11px] font-semibold px-3 py-1.5 rounded-md" style={{ background: "hsl(var(--destructive))", color: "white", opacity: resetting ? 0.6 : 1 }}>
                  {resetting ? "Resetting…" : "Yes, reset everything"}
                </button>
                <button onClick={() => setShowResetConfirm(false)} disabled={resetting}
                  className="text-[11px] font-medium px-3 py-1.5 rounded-md" style={{ background: "hsl(var(--surface-2))", color: "hsl(var(--foreground))" }}>
                  Cancel
                </button>
              </div>
            </div>
          </div>
        )}

        {/* ── Body ── */}
        {loading ? (
          <VocabularySkeleton />
        ) : empty ? (
          <div className="flex items-center justify-center py-16">
            <div className="text-center px-8">
              <div className="w-12 h-12 rounded-full flex items-center justify-center mx-auto mb-4" style={{ background: "hsl(var(--primary) / 0.15)" }}>
                <BookOpen size={20} style={{ color: "hsl(var(--chip-lime-fg))" }} />
              </div>
              <p className="text-[14px] font-semibold text-foreground mb-1">Nothing learned yet</p>
              <p className="text-[12px] text-muted-foreground max-w-xs leading-relaxed">
                Dictate, and when AirNote mishears a name just fix it — it learns the correction and
                auto-applies it next time. Or add a word above to get started.
              </p>
            </div>
          </div>
        ) : (
          <>
            {/* Toolbar */}
            <div className="flex items-center gap-2 mb-4 flex-wrap">
              <div className="flex items-center gap-2 flex-1 min-w-[180px] px-3 h-8 rounded-lg" style={{ background: "hsl(var(--surface-4))", boxShadow: "inset 0 0 0 1px hsl(var(--border))" }}>
                <Search size={13} className="text-muted-foreground flex-shrink-0" />
                <input value={search} onChange={(e) => setSearch(e.target.value)} placeholder="Search words or mishearings…"
                  className="flex-1 bg-transparent outline-none text-[12.5px] text-foreground placeholder:text-muted-foreground/70" />
                {search.length > 0 && <button onClick={() => setSearch("")} className="text-muted-foreground hover:text-foreground transition-colors" title="Clear search"><X size={12} /></button>}
              </div>
              <StatusChips value={statusFilter} counts={counts} onChange={setStatusFilter} />
              <SortDropdown value={sort} onChange={setSort} />
              <button onClick={() => void refresh()} title="Refresh"
                className="w-8 h-8 rounded-lg flex items-center justify-center transition-colors"
                style={{ background: "hsl(var(--surface-4))", color: "hsl(var(--muted-foreground))", boxShadow: "inset 0 0 0 1px hsl(var(--border))" }}>
                <RotateCw size={13} />
              </button>
            </div>

            {noResults ? (
              <div className="text-center py-12">
                <p className="text-[13px] text-muted-foreground mb-3">
                  {q ? <>Nothing matches “{search}”.</> : <>No {statusFilter} terms.</>}
                </p>
                <button onClick={() => { setSearch(""); setStatusFilter("all"); }}
                  className="text-[12px] font-medium px-3 py-1.5 rounded-lg" style={{ background: "hsl(var(--surface-4))", color: "hsl(var(--foreground))" }}>
                  Clear filters
                </button>
              </div>
            ) : sectioned ? (
              <>
                {active.length > 0 && (
                  <Section label="Correcting" hint="fixing your dictation now" count={active.length}>{renderCards(active)}</Section>
                )}
                {inactive.length > 0 && (
                  <Section label="Not yet active" hint="no fix learned — correct once in use" count={inactive.length}>{renderCards(inactive)}</Section>
                )}
              </>
            ) : (
              <div className="space-y-2">{renderCards(filtered)}</div>
            )}
          </>
        )}
      </div>

      {detailInfo && (
        <VocabDetailModal
          info={detailInfo}
          onClose={() => setDetailTerm(null)}
          onSaved={() => {
            invalidateVocabularyCache();
            void refresh(true);
            push({ kind: "success", title: "Saved", duration: 2500 });
          }}
          onError={(m) => push({ kind: "error", title: "Couldn’t save", sub: m, duration: 5000 })}
          onStar={handleStar}
          onDelete={handleDelete}
        />
      )}

      <Toaster toasts={toasts} onDismiss={dismiss} />
    </ScrollArea>
  );
}
