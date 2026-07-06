import { useEffect, useMemo, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { apiJson } from '../../api'
import { TelemetryStatCard } from './TelemetryStatCard'
import { pct, ms } from './format'
import { Loading } from '../States'
import type {
  AliasLearnEvent,
  DictationDetailItem,
  DictationListItem,
  DictationTrace,
  DictationTraceStage,
  ObservabilitySummary,
} from '../../types'

export type AccountLabel = { name: string; sub?: string }
export type ResolveAccount = (accountId: string) => AccountLabel | undefined

function SectionLabel({ children }: { children: React.ReactNode }) {
  return (
    <div className="text-[10px] font-semibold text-fg-4 uppercase tracking-wider mb-3">{children}</div>
  )
}

function DiffColumn({ label, text, tone }: { label: string; text?: string | null; tone?: 'raw' | 'polished' | 'kept' }) {
  const accent =
    tone === 'polished' ? 'text-accent' : tone === 'kept' ? 'text-ok' : 'text-fg-4'
  return (
    <div className="min-w-0">
      <div className={`text-[10px] font-semibold uppercase tracking-wider mb-1.5 ${accent}`}>{label}</div>
      <div className="text-[12px] leading-relaxed whitespace-pre-wrap break-words font-mono bg-surface-3 rounded-lg p-3 min-h-[4rem] border border-border-light">
        {text?.trim() ? text : '—'}
      </div>
    </div>
  )
}

function rowKey(row: DictationListItem): string {
  // Always use the Postgres history row id — guaranteed to match list + detail.
  return row.id
}

function rowLabel(row: DictationListItem): string {
  if (row.recording_id) return `${row.recording_id.slice(0, 8)}… (rec)`
  if (row.client_run_id) return `${row.client_run_id.slice(0, 8)}… (run)`
  return `${row.id.slice(0, 8)}…`
}

// Map the telemetry STT provider/model to a cloud-vs-local badge. provider is the
// internal id ("deepgram" for cloud Whisper, "whisper_local"/"swift_local" on
// device); fall back to the model string when provider is missing.
function sttBadge(
  provider?: string | null,
  model?: string | null,
): { text: string; cloud: boolean } | null {
  const p = (provider || '').toLowerCase()
  const m = (model || '').toLowerCase()
  if (p.includes('whisper_local') || p.includes('swift') || m.includes('apex') || m.includes('oriserve'))
    return { text: 'Local Native', cloud: false }
  if (
    p.includes('deepgram') ||
    p.includes('groq') ||
    p.includes('openrouter') ||
    p.includes('nova') ||
    m.includes('whisper')
  )
    return { text: 'Cloud Whisper', cloud: true }
  if (p) return { text: provider as string, cloud: true }
  return null
}

function isTrace(value: DictationDetailItem['dictation_trace_json']): value is DictationTrace {
  return !!value && Array.isArray((value as DictationTrace).stages)
}

function traceTextMeta(trace: DictationTrace, ref?: string | null) {
  if (!ref) return null
  return trace.texts?.[ref] || null
}

function compactMeta(meta?: Record<string, unknown>): string {
  if (!meta || !Object.keys(meta).length) return ''
  return Object.entries(meta)
    .slice(0, 5)
    .map(([key, value]) => `${key}=${typeof value === 'string' ? value : JSON.stringify(value)}`)
    .join(' · ')
}

function isPrimaryServerRuntimeModel(model?: string | null): boolean {
  return !!model && (model === 'server-runtime' || model.startsWith('server-runtime:'))
}

function isServerRuntimeFallbackModel(model?: string | null): boolean {
  return !!model && model.startsWith('server-runtime-fallback:')
}

function isBackendPromptStage(stage: DictationTraceStage): boolean {
  return (
    stage.component === 'backend' &&
    ['prompt.build', 'prompt.final', 'fallback_prompt.build', 'fallback_prompt.final'].includes(stage.stage)
  )
}

function isFallbackOnlyPromptStage(stage: DictationTraceStage, modelUsed?: string | null): boolean {
  return (
    (stage.metadata?.fallback_only === true && !isServerRuntimeFallbackModel(modelUsed)) ||
    (isPrimaryServerRuntimeModel(modelUsed) && isBackendPromptStage(stage))
  )
}

function cappedWords(text: string, maxWords: number): { text: string; truncated: boolean } {
  const words = text.trim().split(/\s+/).filter(Boolean)
  if (words.length <= maxWords) return { text, truncated: false }
  return { text: `${words.slice(0, maxWords).join(' ')}…`, truncated: true }
}

function TraceTextBlock({
  label,
  text,
  chars,
  words,
  tone,
  wordCap = 140,
}: {
  label: string
  text?: string | null
  chars?: number | null
  words?: number | null
  tone?: 'raw' | 'polished'
  wordCap?: number
}) {
  const [expanded, setExpanded] = useState(false)
  const accent = tone === 'polished' ? 'text-accent' : 'text-fg-4'
  const raw = text?.trim() ? text : '—'
  const capped = raw === '—' ? { text: raw, truncated: false } : cappedWords(raw, wordCap)
  const visible = expanded ? raw : capped.text
  const displayWords = words ?? (raw === '—' ? 0 : raw.trim().split(/\s+/).filter(Boolean).length)
  const displayChars = chars ?? (raw === '—' ? 0 : raw.length)

  return (
    <div className="min-w-0">
      <div className="flex items-center justify-between gap-2 mb-1.5">
        <div className={`text-[10px] font-semibold uppercase tracking-wider ${accent}`}>{label}</div>
        {raw !== '—' && (
          <div className="text-[10px] text-fg-5 font-mono shrink-0">
            {displayWords}w · {displayChars}c
          </div>
        )}
      </div>
      <div className="text-[12px] leading-relaxed whitespace-pre-wrap break-words font-mono bg-surface-3 rounded-lg p-3 min-h-[4rem] border border-border-light">
        {visible}
      </div>
      {capped.truncated && (
        <button
          type="button"
          onClick={() => setExpanded(v => !v)}
          className="mt-2 text-[11px] font-medium text-accent hover:text-accent/80"
        >
          {expanded ? 'Show less' : 'Read more'}
        </button>
      )}
    </div>
  )
}

function suspectStages(trace: DictationTrace): DictationTraceStage[] {
  return trace.stages.filter(
    stage =>
      stage.changed &&
      (stage.risk === 'post_model_mutation' ||
        stage.risk === 'paste_reconcile' ||
        stage.risk === 'edit_capture' ||
        stage.risk === 'learning_classification'),
  )
}

function TraceStageCard({
  trace,
  stage,
  modelUsed,
}: {
  trace: DictationTrace
  stage: DictationTraceStage
  modelUsed?: string | null
}) {
  const inputMeta = traceTextMeta(trace, stage.input_ref)
  const outputMeta = traceTextMeta(trace, stage.output_ref)
  const input = inputMeta?.text || null
  const output = outputMeta?.text || null
  const meta = compactMeta(stage.metadata)
  const fallbackOnly = isFallbackOnlyPromptStage(stage, modelUsed)
  const stageName =
    fallbackOnly && stage.stage.startsWith('prompt.') ? `legacy_fallback_${stage.stage}` : stage.stage
  const reason = fallbackOnly
    ? 'Local backend fallback prompt only. For server-runtime runs, the active model prompt is built in control-plane.'
    : stage.reason

  return (
    <div
      className={`rounded-xl border p-4 ${
        stage.changed ? 'border-accent/40 bg-accent-light/20' : 'border-border-light bg-surface-3'
      }`}
    >
      <div className="flex flex-wrap items-start justify-between gap-3 mb-3">
        <div className="min-w-0">
          <div className="text-[12px] font-mono text-fg break-words">
            {stage.index + 1}. {stageName}
          </div>
          <div className="text-[11px] text-fg-4 break-words">
            {stage.component} · {stage.function}
          </div>
        </div>
        <div className="flex flex-wrap items-center gap-1.5 shrink-0">
          {fallbackOnly && (
            <span className="text-[10px] rounded bg-warn-bg px-2 py-0.5 text-warn">fallback only</span>
          )}
          {stage.risk && <span className="text-[10px] rounded bg-surface-4 px-2 py-0.5 text-fg-4">{stage.risk}</span>}
          {stage.changed && <span className="text-[10px] rounded bg-live/10 px-2 py-0.5 text-live">changed</span>}
          {stage.duration_ms != null && (
            <span className="text-[10px] rounded bg-surface-4 px-2 py-0.5 text-fg-4 tabular-nums">
              {ms(stage.duration_ms)}
            </span>
          )}
        </div>
      </div>
      {reason && <div className="text-[11px] text-fg-4 mb-3">{reason}</div>}
      {fallbackOnly && (input || output) ? (
        <div className="rounded-lg border border-warn/30 bg-warn-bg p-3 text-[11px] text-warn">
          Prompt body hidden because this was not the active model prompt for this server-runtime run.
          Check Context applied for the actual runtime profile/bucket context.
        </div>
      ) : (input || output) ? (
        <div className="grid grid-cols-1 lg:grid-cols-2 gap-3">
          <TraceTextBlock
            label="Before"
            text={input}
            chars={inputMeta?.chars}
            words={inputMeta?.words}
            tone="raw"
            wordCap={120}
          />
          <TraceTextBlock
            label="After"
            text={output}
            chars={outputMeta?.chars}
            words={outputMeta?.words}
            tone={stage.changed ? 'polished' : 'raw'}
            wordCap={120}
          />
        </div>
      ) : null}
      {meta && <div className="text-[10px] text-fg-5 font-mono mt-3 break-words">{meta}</div>}
    </div>
  )
}

function TraceModal({
  trace,
  modelUsed,
  onClose,
}: {
  trace: DictationTrace
  modelUsed?: string | null
  onClose: () => void
}) {
  const scrollRef = useRef<HTMLDivElement | null>(null)
  const suspect = suspectStages(trace)
  const changedCount = trace.stages.filter(stage => stage.changed).length
  const postMutationCount = trace.stages.filter(stage => stage.risk === 'post_model_mutation').length

  useEffect(() => {
    scrollRef.current?.scrollTo({ top: 0 })
    const previousOverflow = document.body.style.overflow
    document.body.style.overflow = 'hidden'
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') onClose()
    }
    window.addEventListener('keydown', onKeyDown)
    return () => {
      document.body.style.overflow = previousOverflow
      window.removeEventListener('keydown', onKeyDown)
    }
  }, [onClose])

  const modal = (
    <div
      className="fixed inset-0 z-[9999] flex items-center justify-center bg-black/65 p-4 backdrop-blur-md"
      role="dialog"
      aria-modal="true"
      onMouseDown={onClose}
    >
      <div
        className="w-[94vw] h-[84vh] lg:w-[70vw] lg:h-[70vh] rounded-2xl border border-border bg-surface-2 shadow-2xl overflow-hidden flex flex-col"
        onMouseDown={event => event.stopPropagation()}
      >
        <div className="px-5 py-4 border-b border-border flex flex-wrap items-center justify-between gap-3">
          <div>
            <div className="text-[11px] font-semibold text-fg uppercase tracking-wider">Detailed dictation trace</div>
            <div className="text-[11px] text-fg-4 mt-1">
              {trace.stages.length} stages · {changedCount} changed · {postMutationCount} post-model mutations
            </div>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="rounded-lg border border-border-light bg-surface-3 px-3 py-1.5 text-[12px] text-fg hover:bg-surface-4"
          >
            Close
          </button>
        </div>
        <div ref={scrollRef} className="flex-1 overflow-auto p-5">
          {suspect.length > 0 && (
            <div className="mb-4 rounded-xl border border-live/30 bg-live/5 p-3">
              <div className="text-[10px] font-semibold text-live uppercase tracking-wider mb-2">Suspect stages</div>
              <div className="flex flex-wrap gap-1.5">
                {suspect.map(stage => (
                  <span key={`${stage.index}-${stage.stage}`} className="text-[10px] font-mono rounded bg-surface-4 px-2 py-1">
                    {stage.stage}
                  </span>
                ))}
              </div>
            </div>
          )}
          <div className="space-y-3">
            {trace.stages.map(stage => (
              <TraceStageCard
                key={`${stage.index}-${stage.stage}`}
                trace={trace}
                stage={stage}
                modelUsed={modelUsed}
              />
            ))}
          </div>
        </div>
      </div>
    </div>
  )

  return createPortal(modal, document.body)
}

const CONTEXT_BUCKET_LABELS: Record<string, string> = {
  coding: 'Coding',
  messaging: 'Messaging',
  work_tracker: 'Work & Tasks',
  formal_writing: 'Formal Writing',
  default: 'General',
}

interface ContextAppliedData {
  bucket_key: string
  bucket_source: string | null
  style: string[]
  global_kb: boolean
  domains?: string[]
  domain_context?: string | null
  domain_source?: string | null
  context_source?: string | null
}

const DOMAIN_SOURCE_LABELS: Record<string, string> = {
  classified: 'learned',
  coding_bucket_seed: 'coding app',
  generic_default: 'generic',
}

const LANGUAGE_LABELS: Record<string, string> = {
  english: 'English',
  hinglish: 'Hinglish',
  hindi: 'Hindi',
}

/** "Context applied" — which app-bucket this run's app resolved to, and the exact
 * (human-authored) style lines that bucket injects into the polish prompt. */
function ContextApplied({ context }: { context?: ContextAppliedData | null }) {
  if (!context) return null
  const label = CONTEXT_BUCKET_LABELS[context.bucket_key] ?? context.bucket_key
  const domains = context.domains ?? []
  const domainSourceLabel = context.domain_source
    ? DOMAIN_SOURCE_LABELS[context.domain_source] ?? context.domain_source
    : null
  return (
    <div className="mb-4">
      <SectionLabel>Context applied</SectionLabel>
      <div className="flex flex-wrap items-center gap-2 mb-2">
        <span className="text-[11px] px-2 py-0.5 rounded-md bg-surface-4 text-fg">
          Bucket: {label}
        </span>
        {context.bucket_source && (
          <span className="text-[11px] px-2 py-0.5 rounded-md bg-surface-3 text-fg-3">
            source: {context.bucket_source}
          </span>
        )}
        <span className="text-[11px] px-2 py-0.5 rounded-md bg-surface-3 text-fg-3">
          KB: {context.global_kb ? 'injected' : 'not injected'}
        </span>
        {context.context_source && (
          <span className="text-[11px] px-2 py-0.5 rounded-md bg-surface-3 text-fg-3">
            trace: {context.context_source === 'runtime_trace' ? 'actual run' : 'current DB fallback'}
          </span>
        )}
      </div>
      <div className="flex flex-wrap items-center gap-2 mb-2">
        <span className="text-[11px] px-2 py-0.5 rounded-md bg-surface-4 text-fg">
          Domain{domainSourceLabel ? ` · ${domainSourceLabel}` : ''}
        </span>
        {domains.length > 0 ? (
          domains.map(d => (
            <span key={d} className="text-[11px] px-2 py-0.5 rounded-md bg-surface-3 text-fg-2">
              {d}
            </span>
          ))
        ) : (
          <span className="text-[11px] px-2 py-0.5 rounded-md bg-surface-3 text-fg-4">
            unclassified (generic)
          </span>
        )}
      </div>
      {context.domain_context && (
        <p className="text-[12px] text-fg-3 mb-2 leading-relaxed">{context.domain_context}</p>
      )}
      {context.style.length > 0 ? (
        <ul className="space-y-1">
          {context.style.map((l, i) => (
            <li key={i} className="text-[12px] text-fg-3">
              · {l}
            </li>
          ))}
        </ul>
      ) : (
        <p className="text-[12px] text-fg-4">No per-app style learned for this bucket yet.</p>
      )}
    </div>
  )
}

function traceNumber(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

function traceBool(value: unknown): boolean | null {
  return typeof value === 'boolean' ? value : null
}

function traceTermName(value: unknown): string | null {
  if (typeof value === 'string') return value
  if (value && typeof value === 'object' && 'term' in value) {
    const term = (value as { term?: unknown }).term
    return typeof term === 'string' ? term : null
  }
  return null
}

type TraceTermDetail = {
  term: string
  detail?: string
  tier?: 'apply' | 'suggest' | string
  reason?: string
  evidence?: string
  score?: number
}

function traceTermDetails(value: unknown): TraceTermDetail | null {
  const term = traceTermName(value)
  if (!term) return null
  if (!value || typeof value !== 'object') return { term }
  const row = value as {
    source?: unknown
    term_type?: unknown
    weight?: unknown
    use_count?: unknown
    has_example_context?: unknown
    has_meaning?: unknown
    tier?: unknown
    reason?: unknown
    evidence?: unknown
    score?: unknown
  }
  const tier = typeof row.tier === 'string' ? row.tier : undefined
  const reason = typeof row.reason === 'string' ? row.reason : undefined
  const evidence = typeof row.evidence === 'string' && row.evidence.trim() ? row.evidence : undefined
  const score = typeof row.score === 'number' && Number.isFinite(row.score) ? row.score : undefined
  const parts = [
    tier ? tier.toUpperCase() : null,
    reason ?? null,
    evidence ? `heard="${evidence}"` : null,
    typeof score === 'number' ? `score=${score.toFixed(2)}` : null,
    typeof row.source === 'string' ? row.source : null,
    typeof row.term_type === 'string' ? row.term_type : null,
    typeof row.weight === 'number' ? `w=${row.weight.toFixed(2)}` : null,
    typeof row.use_count === 'number' ? `use=${row.use_count}` : null,
    row.has_example_context === true ? 'ctx' : null,
    row.has_meaning === true ? 'meaning' : null,
  ].filter(Boolean)
  return { term, detail: parts.join(' · ') || undefined, tier, reason, evidence, score }
}

function traceTermDetailsList(value: unknown): TraceTermDetail[] {
  if (!Array.isArray(value)) return []
  return value.map(traceTermDetails).filter((item): item is TraceTermDetail => !!item)
}

function findVocabStage(trace?: DictationTrace | Record<string, never>): DictationTraceStage | null {
  if (!isTrace(trace)) return null
  return (
    trace.stages.find(stage => stage.stage === 'vocab.select_for_prompt') ??
    trace.stages.find(stage => stage.stage === 'vocab.resolve_for_prompt') ??
    null
  )
}

function findRuntimePromptStage(trace?: DictationTrace | Record<string, never>): DictationTraceStage | null {
  if (!isTrace(trace)) return null
  return trace.stages.find(stage => stage.stage === 'server_runtime.prompt_built') ?? null
}

function TermPills({
  terms,
  empty,
}: {
  terms: TraceTermDetail[]
  empty: string
}) {
  if (!terms.length) return <p className="text-[11px] text-fg-5">{empty}</p>
  return (
    <div className="flex flex-wrap gap-1.5">
      {terms.slice(0, 30).map((term, index) => {
        const tone =
          term.tier === 'apply'
            ? 'bg-ok/10 text-ok border-ok/20'
            : term.tier === 'suggest'
              ? 'bg-warn-bg text-warn border-warn/20'
              : 'bg-surface-4 text-fg-3 border-transparent'
        return (
          <span
            key={`${term.term}-${index}`}
            title={term.detail}
            className={`rounded-md border px-2 py-1 text-[10px] font-mono ${tone}`}
          >
            {term.term}
          </span>
        )
      })}
      {terms.length > 30 && <span className="text-[10px] text-fg-5">+{terms.length - 30} more</span>}
    </div>
  )
}

function VocabularyLifecycle({ trace }: { trace?: DictationTrace | Record<string, never> }) {
  const stage = findVocabStage(trace)
  const runtimePromptStage = findRuntimePromptStage(trace)
  const meta = stage?.metadata ?? {}
  const runtimeMeta = runtimePromptStage?.metadata ?? {}
  const selectorTerms = traceTermDetailsList(meta.selector_terms)
  const applyTerms =
    selectorTerms.filter(term => term.tier === 'apply').length > 0
      ? selectorTerms.filter(term => term.tier === 'apply')
      : traceTermDetailsList(meta.apply_terms).map(term => ({ ...term, tier: 'apply' }))
  const suggestTerms =
    selectorTerms.filter(term => term.tier === 'suggest').length > 0
      ? selectorTerms.filter(term => term.tier === 'suggest')
      : traceTermDetailsList(meta.suggest_terms).map(term => ({ ...term, tier: 'suggest' }))
  const selectedTerms = traceNumber(meta.selected_terms) ?? 0
  const savedTotal = traceNumber(meta.saved_terms_total)
  const selectorCount =
    traceNumber(meta.selector_terms_after_company_count) ??
    traceNumber(meta.sent_to_prompt_count) ??
    (selectorTerms.length || null)
  const resolvedCount = traceNumber(meta.apply_terms_count) ?? traceNumber(meta.resolved_terms_count) ?? applyTerms.length
  const candidateCount = traceNumber(meta.suggest_terms_count) ?? traceNumber(meta.candidate_terms_count) ?? suggestTerms.length
  const sentCount = traceNumber(meta.sent_to_prompt_count) ?? selectedTerms
  const droppedCount = traceNumber(meta.dropped_candidate_count)
  const aliasMatches = traceNumber(meta.resolver_alias_matches)
  const contextMatches = traceNumber(meta.resolver_context_matches)
  const companyAdded = traceNumber(meta.company_terms_added_count)
  const companyAvailable = traceNumber(meta.company_terms_available)
  const replacementRules = traceNumber(meta.stt_replacement_rules)
  const embeddingHit = traceBool(meta.embedding_cache_hit)
  const sentTerms =
    selectorTerms.length > 0
      ? selectorTerms
      : traceTermDetailsList(meta.sent_to_prompt_terms).length > 0
        ? traceTermDetailsList(meta.sent_to_prompt_terms)
        : traceTermDetailsList(meta.terms)
  const candidateTerms = traceTermDetailsList(meta.candidate_terms)
  const companyTerms = traceTermDetailsList(meta.company_terms_added)
  const runtimeVocabHints = traceNumber(runtimeMeta.vocab_hints)
  const runtimeApplyTerms = traceNumber(runtimeMeta.apply_vocab_terms)
  const newPipeline = stage?.stage === 'vocab.select_for_prompt'

  return (
    <div className="mb-4">
      <SectionLabel>Vocabulary lifecycle</SectionLabel>
      {!stage ? (
        <p className="text-[12px] text-fg-4">No vocabulary filtering trace captured for this dictation.</p>
      ) : (
        <div className="rounded-xl border border-border-light bg-surface-3 p-4 space-y-4">
          <div className="grid grid-cols-2 md:grid-cols-4 gap-2">
            <div className="rounded-lg bg-surface-4 p-3">
              <div className="text-[10px] uppercase text-fg-5 mb-1">Saved terms</div>
              <div className="text-[16px] font-semibold text-fg">{savedTotal ?? '—'}</div>
            </div>
            <div className="rounded-lg bg-surface-4 p-3">
              <div className="text-[10px] uppercase text-fg-5 mb-1">Selector output</div>
              <div className="text-[16px] font-semibold text-fg">{selectorCount ?? '—'}</div>
            </div>
            <div className="rounded-lg bg-surface-4 p-3">
              <div className="text-[10px] uppercase text-fg-5 mb-1">APPLY</div>
              <div className="text-[16px] font-semibold text-ok">{resolvedCount}</div>
            </div>
            <div className="rounded-lg bg-surface-4 p-3">
              <div className="text-[10px] uppercase text-fg-5 mb-1">SUGGEST</div>
              <div className="text-[16px] font-semibold text-warn">{candidateCount}</div>
            </div>
            <div className="rounded-lg bg-surface-4 p-3">
              <div className="text-[10px] uppercase text-fg-5 mb-1">Sent to prompt</div>
              <div className="text-[16px] font-semibold text-ok">{sentCount}</div>
            </div>
            <div className="rounded-lg bg-surface-4 p-3">
              <div className="text-[10px] uppercase text-fg-5 mb-1">{newPipeline ? 'Filtered out' : 'Dropped candidates'}</div>
              <div className="text-[16px] font-semibold text-fg">{droppedCount ?? candidateCount ?? '—'}</div>
            </div>
          </div>
          <div className="flex flex-wrap gap-2 text-[10px] text-fg-4">
            <span className="rounded bg-surface-4 px-2 py-1">
              selector: {newPipeline ? 'tiered v3' : 'legacy resolver'}
            </span>
            <span className="rounded bg-surface-4 px-2 py-1">embedding: {embeddingHit == null ? 'unknown' : embeddingHit ? 'hit' : 'miss'}</span>
            {runtimePromptStage && (
              <span className="rounded bg-surface-4 px-2 py-1">
                runtime prompt: {runtimeVocabHints ?? '—'} hints / {runtimeApplyTerms ?? '—'} apply
              </span>
            )}
            <span className="rounded bg-surface-4 px-2 py-1">STT alias rules: {replacementRules ?? '—'}</span>
            <span className="rounded bg-surface-4 px-2 py-1">company available: {companyAvailable ?? '—'}</span>
            <span className="rounded bg-surface-4 px-2 py-1">company added: {companyAdded ?? 0}</span>
            {!newPipeline && <span className="rounded bg-surface-4 px-2 py-1">alias matches: {aliasMatches ?? '—'}</span>}
            {!newPipeline && <span className="rounded bg-surface-4 px-2 py-1">context matches: {contextMatches ?? '—'}</span>}
          </div>
          <div>
            <div className="text-[10px] font-semibold uppercase tracking-wider text-ok mb-2">APPLY, normalize directly</div>
            <TermPills terms={applyTerms} empty="No direct-apply vocab terms." />
          </div>
          <div>
            <div className="text-[10px] font-semibold uppercase tracking-wider text-warn mb-2">SUGGEST, model decides from context</div>
            <TermPills terms={suggestTerms} empty="No suggest-tier vocab terms." />
          </div>
          <div>
            <div className="text-[10px] font-semibold uppercase tracking-wider text-ok mb-2">Sent to model prompt</div>
            <TermPills terms={sentTerms} empty="No vocab terms reached the polish prompt." />
          </div>
          {!newPipeline && (
            <div>
              <div className="text-[10px] font-semibold uppercase tracking-wider text-fg-4 mb-2">Candidate but not sent</div>
              <TermPills terms={candidateTerms} empty="No dropped vocab candidates." />
            </div>
          )}
          <div>
            <div className="text-[10px] font-semibold uppercase tracking-wider text-fg-4 mb-2">
              {newPipeline ? 'Selector evidence rows' : 'Selector pool'}
            </div>
            <TermPills terms={selectorTerms} empty="No selector candidates." />
          </div>
          {companyTerms.length > 0 && (
            <div>
              <div className="text-[10px] font-semibold uppercase tracking-wider text-fg-4 mb-2">Company additions</div>
              <TermPills terms={companyTerms} empty="No company terms added." />
            </div>
          )}
        </div>
      )}
    </div>
  )
}

function TraceTimeline({
  trace,
  modelUsed,
}: {
  trace?: DictationTrace | Record<string, never>
  modelUsed?: string | null
}) {
  const [modalOpen, setModalOpen] = useState(false)

  if (!isTrace(trace) || !trace.stages.length) {
    return (
      <div className="mb-4">
        <SectionLabel>Trace timeline</SectionLabel>
        <p className="text-[12px] text-fg-4">No detailed trace captured for this dictation.</p>
      </div>
    )
  }

  const suspect = suspectStages(trace)
  const changedCount = trace.stages.filter(stage => stage.changed).length
  const postMutationCount = trace.stages.filter(stage => stage.risk === 'post_model_mutation').length
  const hasFallbackOnlyPrompt = trace.stages.some(stage => isFallbackOnlyPromptStage(stage, modelUsed))

  return (
    <div className="mb-4">
      <SectionLabel>Trace timeline</SectionLabel>
      <div className="rounded-xl border border-border-light bg-surface-3 p-4">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div>
            <div className="text-[12px] font-medium text-fg">Detailed trace captured</div>
            <div className="text-[11px] text-fg-4 mt-1">
              {trace.stages.length} stages · {changedCount} changed · {postMutationCount} post-model mutations
            </div>
          </div>
          <button
            type="button"
            onClick={() => setModalOpen(true)}
            className="rounded-lg bg-accent px-3 py-2 text-[12px] font-semibold text-white hover:bg-accent/90"
          >
            Show me the trace, detailed trace
          </button>
        </div>
        {hasFallbackOnlyPrompt && (
          <div className="mt-3 rounded-lg border border-warn/30 bg-warn-bg p-3 text-[11px] text-warn">
            Backend prompt stages in this trace are local fallback prompts. The active server-runtime context is shown in Context applied.
          </div>
        )}
        {suspect.length > 0 && (
          <div className="mt-3 rounded-lg border border-live/30 bg-live/5 p-3">
            <div className="text-[10px] font-semibold text-live uppercase tracking-wider mb-2">Suspect stages</div>
            <div className="flex flex-wrap gap-1.5">
              {suspect.map(stage => (
                <span key={`${stage.index}-${stage.stage}`} className="text-[10px] font-mono rounded bg-surface-4 px-2 py-1">
                  {stage.stage}
                </span>
              ))}
            </div>
          </div>
        )}
      </div>
      {modalOpen && <TraceModal trace={trace} modelUsed={modelUsed} onClose={() => setModalOpen(false)} />}
    </div>
  )
}

/**
 * Self-contained STT → polish → kept inspector.
 * - No `accountId`  → org-wide firehose (adds a "Who" column via `resolveAccount`).
 * - With `accountId` → scoped to one member (per-user detail tab).
 */
export function DictationInspector({
  orgId,
  accountId,
  days,
  resolveAccount,
  focusKey,
}: {
  orgId: string
  accountId?: string
  days: number
  resolveAccount?: ResolveAccount
  focusKey?: string | null
}) {
  const orgWide = !accountId

  const [items, setItems] = useState<DictationListItem[]>([])
  const [total, setTotal] = useState(0)
  const [summary, setSummary] = useState<ObservabilitySummary | null>(null)
  const [loading, setLoading] = useState(false)
  const [query, setQuery] = useState('')

  const [selectedKey, setSelectedKey] = useState<string | null>(focusKey ?? null)
  const [detail, setDetail] = useState<{ item: DictationDetailItem; alias_events: AliasLearnEvent[]; context_applied?: ContextAppliedData | null } | null>(null)
  const [detailLoading, setDetailLoading] = useState(false)
  const [detailError, setDetailError] = useState<string | null>(null)

  // Cross-surface focus (e.g. "Open dictation inspector →" from a run row).
  useEffect(() => {
    if (focusKey) setSelectedKey(focusKey)
  }, [focusKey])

  useEffect(() => {
    if (!orgId) return
    setLoading(true)
    const params = new URLSearchParams({ days: String(days), limit: '100' })
    if (accountId) params.set('account_id', accountId)
    Promise.all([
      apiJson<{ items: DictationListItem[]; total: number }>(
        `/v1/orgs/${orgId}/observability/dictation?${params}`,
      ),
      apiJson<ObservabilitySummary>(`/v1/orgs/${orgId}/observability/summary?days=${days}`),
    ])
      .then(([list, s]) => {
        setItems(list.items || [])
        setTotal(list.total || 0)
        setSummary(s)
      })
      .catch(() => {
        setItems([])
        setTotal(0)
        setSummary(null)
      })
      .finally(() => setLoading(false))
  }, [orgId, accountId, days])

  useEffect(() => {
    if (!orgId || !selectedKey) {
      setDetail(null)
      return
    }
    setDetailLoading(true)
    setDetailError(null)
    const suffix = accountId ? `?account_id=${accountId}` : ''
    apiJson<{ item: DictationDetailItem; alias_events: AliasLearnEvent[]; context_applied?: ContextAppliedData | null }>(
      `/v1/orgs/${orgId}/observability/dictation/${encodeURIComponent(selectedKey)}${suffix}`,
    )
      .then(setDetail)
      .catch(e => {
        setDetail(null)
        setDetailError(e instanceof Error ? e.message : 'Failed to load dictation detail')
      })
      .finally(() => setDetailLoading(false))
  }, [orgId, accountId, selectedKey])

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase()
    if (!q) return items
    return items.filter(row => {
      const who = orgWide ? resolveAccount?.(row.account_id) : undefined
      return (
        (who?.name || '').toLowerCase().includes(q) ||
        (who?.sub || '').toLowerCase().includes(q) ||
        (row.target_app || '').toLowerCase().includes(q) ||
        (row.recording_id || '').toLowerCase().includes(q) ||
        (row.edit_bucket || '').toLowerCase().includes(q)
      )
    })
  }, [items, query, orgWide, resolveAccount])

  const isSelected = (row: DictationListItem) =>
    selectedKey === rowKey(row) ||
    (!!row.recording_id && selectedKey === row.recording_id) ||
    (!!row.client_run_id && selectedKey === row.client_run_id)

  return (
    <>
      {loading ? (
        <Loading />
      ) : (
        <>
          <div className="grid grid-cols-2 md:grid-cols-5 gap-3 mb-4">
            <TelemetryStatCard label="Dictations" value={String(summary?.dictation_count ?? total)} sub={`${days}d window`} />
            <TelemetryStatCard label="Edits logged" value={String(summary?.edits_detected ?? 0)} sub="with classify feedback" />
            <TelemetryStatCard
              label="STT-error edits"
              value={String(summary?.stt_error_edits ?? 0)}
              sub={pct((summary?.classify_stt_error_rate ?? 0) * 100)}
            />
            <TelemetryStatCard label="Aliases learned" value={String(summary?.aliases_learned ?? 0)} sub="org-wide" />
            <TelemetryStatCard label="Listed" value={String(filtered.length)} sub={query ? 'filtered' : 'rows below'} />
          </div>

          <div className="grid grid-cols-1 xl:grid-cols-2 gap-4">
            <div className="card !p-0 overflow-hidden self-start">
              <div className="px-5 py-3 border-b border-border flex items-center justify-between gap-3">
                <SectionLabel>{orgWide ? 'All dictation runs' : 'Dictation runs'}</SectionLabel>
                <input
                  value={query}
                  onChange={e => setQuery(e.target.value)}
                  placeholder={orgWide ? 'Filter by who, app, id…' : 'Filter by app, id…'}
                  className="text-[11px] bg-surface-3 border border-border-light rounded-md px-2 py-1 w-40 focus:outline-none focus:border-accent"
                />
              </div>
              <div className="max-h-[32rem] overflow-auto">
                <table className="w-full">
                  <thead className="sticky top-0 bg-surface-2 z-10">
                    <tr>
                      {[...(orgWide ? ['Who'] : []), 'When', 'recording_id', 'App', 'Lang', 'STT', 'Words', 'Edit', ''].map(h => (
                        <th
                          key={h}
                          className="text-[10px] font-medium text-fg-4 text-left px-4 py-2 border-b border-border uppercase"
                        >
                          {h}
                        </th>
                      ))}
                    </tr>
                  </thead>
                  <tbody>
                    {filtered.map(row => {
                      const who = orgWide ? resolveAccount?.(row.account_id) : undefined
                      return (
                        <tr
                          key={row.id}
                          className={`cursor-pointer hover:bg-surface-4/30 ${isSelected(row) ? 'bg-accent-light/40' : ''}`}
                          onClick={() => setSelectedKey(rowKey(row))}
                        >
                          {orgWide && (
                            <td className="text-[11px] px-4 py-2 border-b border-border-light truncate max-w-[9rem]">
                              <div className="text-fg font-medium truncate">{who?.name || `${row.account_id.slice(0, 8)}…`}</div>
                              {who?.sub && <div className="text-fg-5 text-[10px] truncate">{who.sub}</div>}
                            </td>
                          )}
                          <td className="text-[11px] px-4 py-2 border-b border-border-light whitespace-nowrap">
                            {new Date(row.created_at).toLocaleString()}
                          </td>
                          <td className="text-[11px] font-mono px-4 py-2 border-b border-border-light truncate max-w-[8rem]">
                            {rowLabel(row)}
                          </td>
                          <td className="text-[11px] px-4 py-2 border-b border-border-light truncate max-w-[6rem]">
                            {row.target_app || '—'}
                          </td>
                          <td className="text-[11px] px-4 py-2 border-b border-border-light whitespace-nowrap">
                            {row.output_language ? LANGUAGE_LABELS[row.output_language] ?? row.output_language : '—'}
                          </td>
                          <td className="text-[11px] px-4 py-2 border-b border-border-light whitespace-nowrap">
                            {(() => {
                              const s = sttBadge(row.stt_provider, row.stt_model)
                              if (!s) return <span className="text-fg-5">—</span>
                              return (
                                <span
                                  className="inline-flex items-center rounded-full px-2 py-0.5 text-[10px] font-medium"
                                  style={
                                    s.cloud
                                      ? { background: 'hsl(210 90% 60% / 0.15)', color: 'hsl(210 90% 74%)' }
                                      : { background: 'hsl(150 60% 45% / 0.16)', color: 'hsl(150 55% 64%)' }
                                  }
                                >
                                  {s.text}
                                </span>
                              )
                            })()}
                          </td>
                          <td className="text-[12px] tabular-nums px-4 py-2 border-b border-border-light">
                            {row.word_count ?? '—'}
                          </td>
                          <td className="text-[11px] px-4 py-2 border-b border-border-light">
                            {row.edit_bucket || (row.has_edit_feedback ? 'edited' : '—')}
                          </td>
                          <td className="text-[11px] px-4 py-2 border-b border-border-light text-fg-4 whitespace-nowrap">
                            {ms(row.total_ms ?? null)}
                          </td>
                        </tr>
                      )
                    })}
                    {!filtered.length && (
                      <tr>
                        <td colSpan={orgWide ? 7 : 6} className="text-[12px] text-fg-4 px-4 py-4">
                          {query
                            ? 'No dictations match your filter.'
                            : 'No dictation history synced yet. Signed-in desktop sessions enqueue plaintext asynchronously.'}
                        </td>
                      </tr>
                    )}
                  </tbody>
                </table>
              </div>
            </div>

            <div className="card p-4 self-start">
              <SectionLabel>Pipeline detail</SectionLabel>
              {!selectedKey ? (
                <p className="text-[12px] text-fg-4">Select a row to inspect the STT → polish → kept stages and edits.</p>
              ) : detailLoading ? (
                <Loading />
              ) : detailError ? (
                <p className="text-[12px] text-live">{detailError}</p>
              ) : !detail ? (
                <p className="text-[12px] text-fg-4">Detail not found for this recording.</p>
              ) : (
                <>
                  <div className="flex flex-wrap items-center gap-2 mb-3">
                    <span className="text-[11px] text-fg-4 font-mono">{selectedKey}</span>
                    {detail.item.target_app && (
                      <span className="text-[10px] px-2 py-0.5 rounded bg-surface-4 text-fg-3">
                        {detail.item.target_app}
                      </span>
                    )}
                    {detail.item.output_language && (
                      <span className="text-[10px] px-2 py-0.5 rounded bg-surface-4 text-fg-2">
                        lang: {LANGUAGE_LABELS[detail.item.output_language] ?? detail.item.output_language}
                      </span>
                    )}
                    {detail.item.model_used && (
                      <span className="text-[10px] px-2 py-0.5 rounded bg-surface-4 text-fg-3 font-mono">
                        {detail.item.model_used}
                      </span>
                    )}
                    {detail.item.total_ms != null && (
                      <span className="text-[10px] px-2 py-0.5 rounded bg-surface-4 text-fg-3 tabular-nums">
                        {ms(detail.item.total_ms)}
                      </span>
                    )}
                  </div>
                  <div className="grid grid-cols-1 gap-3 mb-4">
                    <DiffColumn label="Raw STT" text={detail.item.raw_transcript} tone="raw" />
                    {detail.item.local_corrected_transcript?.trim() &&
                    detail.item.local_corrected_transcript !== detail.item.transcript ? (
                      <DiffColumn label="Local corrected" text={detail.item.local_corrected_transcript} tone="raw" />
                    ) : null}
                    <DiffColumn label="Transcript" text={detail.item.transcript} tone="raw" />
                    <DiffColumn label="Polished" text={detail.item.polished_output} tone="polished" />
                    <DiffColumn label="User kept" text={detail.item.final_text} tone="kept" />
                  </div>
                  <ContextApplied context={detail.context_applied} />
                  <VocabularyLifecycle trace={detail.item.dictation_trace_json} />
                  <TraceTimeline
                    trace={detail.item.dictation_trace_json}
                    modelUsed={detail.item.model_used}
                  />
                  {detail.item.edit_feedback_json &&
                  Object.keys(detail.item.edit_feedback_json).length > 0 ? (
                    <div className="mb-4">
                      <SectionLabel>Edit feedback</SectionLabel>
                      <div className="text-[11px] space-y-1">
                        {Array.isArray(detail.item.edit_feedback_json.changes) &&
                          (detail.item.edit_feedback_json.changes as { type?: string; from?: string; to?: string }[]).map(
                            (ch, i) => (
                              <div key={i} className="font-mono bg-surface-3 rounded px-2 py-1">
                                <span className="text-fg-4">{ch.type || 'change'}</span>{' '}
                                <span className="text-live">{ch.from}</span> → <span className="text-ok">{ch.to}</span>
                              </div>
                            ),
                          )}
                      </div>
                    </div>
                  ) : null}
                  {detail.alias_events.length > 0 && (
                    <div>
                      <SectionLabel>Alias learn timeline</SectionLabel>
                      <div className="text-[11px] space-y-1">
                        {detail.alias_events.map(ev => (
                          <div key={ev.id} className="flex justify-between gap-2 font-mono">
                            <span>
                              {ev.heard} → {ev.correct}
                            </span>
                            <span className="text-fg-4 shrink-0">{ev.source}</span>
                          </div>
                        ))}
                      </div>
                    </div>
                  )}
                </>
              )}
            </div>
          </div>
        </>
      )}
    </>
  )
}
