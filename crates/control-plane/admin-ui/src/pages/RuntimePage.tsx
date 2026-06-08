import { useEffect, useMemo, useState } from 'react'
import { ChevronRight, Check, Clock, RefreshCw, Search, Server, Timer, XCircle } from 'lucide-react'
import { apiJson } from '../api'
import { Badge, MetricCard, StageTimeline } from '../components/charts'
import { formatIstDateTime, timeAgo } from '../utils'

type Tab = 'logs' | 'learning' | 'overview'
type LogFilter = 'all' | 'completed' | 'failed'

interface RuntimeRunSummary {
  id: string
  account_id: string
  account_email: string
  client_run_id?: string | null
  mode: string
  source: string
  platform?: string | null
  app_version?: string | null
  status: string
  error_kind?: string | null
  input_hash?: string | null
  output_hash?: string | null
  provider_summary: Record<string, unknown>
  latency_json: Record<string, unknown>
  metadata_json: Record<string, unknown>
  created_at: string
  updated_at: string
}

interface RuntimeStageSummary {
  id: string
  stage: string
  status: string
  latency_ms: number | null
  error_kind?: string | null
  metadata_json: Record<string, unknown>
  created_at: string
}

interface RuntimeProviderUsageSummary {
  id: string
  provider: string
  model?: string | null
  credential_scope: string
  request_ms?: number | null
  ttft_ms?: number | null
  stream_ms?: number | null
  total_ms?: number | null
  timeout_ms?: number | null
  status: string
  error_kind?: string | null
  fallback_reason?: string | null
  created_at: string
}

interface RuntimeRunDetail {
  run: RuntimeRunSummary
  stages: RuntimeStageSummary[]
  provider_usage: RuntimeProviderUsageSummary[]
}

interface RuntimeLearningEventSummary {
  id: string
  account_id: string
  account_email: string
  run_id?: string | null
  recording_id?: string | null
  event_type: string
  classification?: string | null
  input_hash?: string | null
  output_hash?: string | null
  corrected_hash?: string | null
  payload_json: Record<string, unknown>
  server_judgment: Record<string, unknown>
  created_at: string
}

function numberFrom(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

function totalLatency(run: RuntimeRunSummary): number | null {
  return (
    numberFrom(run.latency_json?.total_ms) ??
    numberFrom(run.latency_json?.total) ??
    numberFrom(run.latency_json?.totalMs)
  )
}

function runMode(run: RuntimeRunSummary): string {
  if (run.mode === 'message_polish') return 'MSG'
  if (run.mode.includes('voice')) return 'VOICE'
  return run.mode.toUpperCase().slice(0, 12)
}

function modelLabel(run: RuntimeRunSummary, detail?: RuntimeRunDetail): string {
  const fromProvider = detail?.provider_usage.find(p => p.model)?.model
  const fromSummary = run.provider_summary?.model || run.provider_summary?.model_used
  const model = String(fromProvider || fromSummary || 'unknown')
  if (model.includes('scout')) return 'Smart Scout'
  if (model.includes('8b') || model.includes('instant')) return 'Fast 8B'
  if (model === 'unknown') return 'Unknown model'
  return model.split('/').pop() || model
}

function shortHash(hash?: string | null): string {
  if (!hash) return '—'
  return hash.length > 14 ? `${hash.slice(0, 10)}…` : hash
}

function safeJson(value: unknown): string {
  if (!value || (typeof value === 'object' && Object.keys(value as object).length === 0)) return '—'
  return JSON.stringify(value, null, 2)
}

export function RuntimePage() {
  const [tab, setTab] = useState<Tab>('logs')
  const [runs, setRuns] = useState<RuntimeRunSummary[]>([])
  const [learningEvents, setLearningEvents] = useState<RuntimeLearningEventSummary[]>([])
  const [details, setDetails] = useState<Record<string, RuntimeRunDetail>>({})
  const [open, setOpen] = useState<Set<string>>(new Set())
  const [filter, setFilter] = useState<LogFilter>('all')
  const [q, setQ] = useState('')
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)

  async function loadRuns() {
    setLoading(true)
    setError(null)
    try {
      const [runData, learningData] = await Promise.all([
        apiJson<RuntimeRunSummary[]>('/v1/runtime/runs?limit=100'),
        apiJson<RuntimeLearningEventSummary[]>('/v1/runtime/learning-events?limit=100'),
      ])
      setRuns(runData)
      setLearningEvents(learningData)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => { loadRuns() }, [])

  async function toggle(id: string) {
    setOpen(prev => {
      const next = new Set(prev)
      next.has(id) ? next.delete(id) : next.add(id)
      return next
    })
    if (!details[id]) {
      try {
        const detail = await apiJson<RuntimeRunDetail>(`/v1/runtime/runs/${id}`)
        setDetails(prev => ({ ...prev, [id]: detail }))
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e))
      }
    }
  }

  const query = q.trim().toLowerCase()
  const filtered = runs.filter(run => {
    const statusOk = filter === 'all' || run.status === filter || (filter === 'failed' && run.status !== 'completed')
    const qOk = !query || [run.id, run.account_id, run.account_email, run.client_run_id, run.mode, run.source, run.status, run.error_kind, run.platform, run.app_version]
      .filter(Boolean)
      .some(v => String(v).toLowerCase().includes(query))
    return statusOk && qOk
  })

  const overview = useMemo(() => {
    const completed = runs.filter(r => r.status === 'completed').length
    const failed = runs.filter(r => r.status !== 'completed').length
    const latencies = runs.map(totalLatency).filter((n): n is number => typeof n === 'number')
    const avg = latencies.length ? Math.round(latencies.reduce((a, b) => a + b, 0) / latencies.length) : 0
    const serverPolish = runs.filter(r => r.source.includes('desktop') || r.source.includes('runtime')).length
    return { completed, failed, avg, serverPolish }
  }, [runs])

  return (
    <>
      <div className="mb-5 flex items-end justify-between gap-4">
        <div>
          <h1 className="text-xl font-semibold tracking-tight">Runtime</h1>
          <p className="text-[12px] text-fg-4 mt-0.5">Live server runtime ledger · privacy-safe hashes and latency traces</p>
        </div>
        <div className="flex items-center gap-2">
          <button
            onClick={loadRuns}
            className="h-8 rounded-lg px-3 text-[12px] font-medium bg-surface-4 text-fg hover:bg-surface-3 inline-flex items-center gap-2"
          >
            <RefreshCw size={13} className={loading ? 'animate-spin' : ''} /> Refresh
          </button>
          <span className="text-[10px] font-semibold px-2.5 py-1 rounded-full bg-surface-4 text-fg-4 uppercase tracking-wide">Live API</span>
        </div>
      </div>

      <div className="mb-4 flex gap-1.5">
        {([['logs', 'Logs'], ['learning', 'Learning'], ['overview', 'Overview']] as [Tab, string][]).map(([id, label]) => (
          <button
            key={id}
            onClick={() => setTab(id)}
            className={`h-8 rounded-lg px-3 text-[12px] font-medium ${tab === id ? 'bg-surface-4 text-fg' : 'text-fg-4 hover:text-fg hover:bg-surface-4/40'}`}
          >
            {label}
          </button>
        ))}
      </div>

      {error && <div className="mb-3 rounded-lg border border-live/30 bg-live/10 px-4 py-3 text-[12px] text-live">{error}</div>}
      {tab === 'overview' ? <Overview runs={runs} overview={overview} /> : tab === 'learning' ? (
        <Learning events={learningEvents} loading={loading} />
      ) : (
        <Logs
          runs={filtered}
          total={runs.length}
          loading={loading}
          open={open}
          details={details}
          filter={filter}
          setFilter={setFilter}
          q={q}
          setQ={setQ}
          toggle={toggle}
        />
      )}
    </>
  )
}

function Overview({ runs, overview }: { runs: RuntimeRunSummary[]; overview: { completed: number; failed: number; avg: number; serverPolish: number } }) {
  const latest = runs[0]
  return (
    <>
      <div className="grid grid-cols-4 gap-4 mb-4">
        <MetricCard label="Runs" value={runs.length.toLocaleString()} icon={<Server size={14} />} />
        <MetricCard label="Completed" value={overview.completed.toLocaleString()} icon={<Check size={14} />} />
        <MetricCard label="Failures" value={overview.failed.toLocaleString()} icon={<XCircle size={14} />} />
        <MetricCard label="Avg latency" value={overview.avg ? `${overview.avg} ms` : '—'} icon={<Timer size={14} />} />
      </div>
      <section className="card">
        <div className="flex items-center justify-between mb-3">
          <span className="text-[13px] font-semibold">Current server runtime state</span>
          <Badge value={overview.failed ? 'watching' : 'healthy'} tone={overview.failed ? 'warn' : 'ok'} />
        </div>
        <div className="grid grid-cols-2 gap-4 text-[12px] text-fg-3">
          <Info label="Latest run" value={latest ? timeAgo(latest.created_at) : 'No runs yet'} />
          <Info label="Latest status" value={latest?.status || '—'} />
          <Info label="Latest source" value={latest?.source || '—'} />
          <Info label="Latest platform" value={[latest?.platform, latest?.app_version].filter(Boolean).join(' · ') || '—'} />
        </div>
      </section>
    </>
  )
}

function Logs(props: {
  runs: RuntimeRunSummary[]
  total: number
  loading: boolean
  open: Set<string>
  details: Record<string, RuntimeRunDetail>
  filter: LogFilter
  setFilter: (f: LogFilter) => void
  q: string
  setQ: (q: string) => void
  toggle: (id: string) => void
}) {
  return (
    <>
      <div className="mb-3 flex items-center justify-between gap-3">
        <div className="flex gap-1.5">
          {([['all', 'All'], ['completed', 'Completed'], ['failed', 'Failed']] as [LogFilter, string][]).map(([id, label]) => (
            <button
              key={id}
              onClick={() => props.setFilter(id)}
              className={`h-8 rounded-lg px-3 text-[12px] font-medium ${props.filter === id ? 'bg-surface-4 text-fg' : 'text-fg-4 hover:text-fg hover:bg-surface-4/40'}`}
            >
              {label}
            </button>
          ))}
        </div>
        <div className="flex items-center gap-3">
          <div className="flex items-center gap-2 h-8 px-3 rounded-lg border border-border bg-[hsla(0,0%,0%,0.25)] text-[12px] text-fg-3 w-[250px] focus-within:border-accent/50">
            <Search size={13} className="opacity-50 shrink-0" />
            <input
              value={props.q}
              onChange={e => props.setQ(e.target.value)}
              placeholder="Search run, source, status…"
              className="bg-transparent outline-none w-full placeholder:text-fg-5"
            />
          </div>
          <span className="text-[11px] text-fg-4 tabular-nums whitespace-nowrap">{props.runs.length} of {props.total}</span>
        </div>
      </div>

      <section className="card !p-0 overflow-hidden">
        <div className="px-5 py-3 border-b border-border flex items-center justify-between">
          <h2 className="text-[13px] font-semibold">Runtime logs</h2>
          <span className="text-[11px] text-fg-4">newest first · raw text hidden</span>
        </div>
        {props.loading ? (
          <div className="px-5 py-12 text-center text-[13px] text-fg-4">Loading runtime logs…</div>
        ) : props.runs.length === 0 ? (
          <div className="px-5 py-12 text-center text-[13px] text-fg-4">No runtime logs yet.</div>
        ) : props.runs.map(run => (
          <LogRow
            key={run.id}
            run={run}
            open={props.open.has(run.id)}
            detail={props.details[run.id]}
            onToggle={() => props.toggle(run.id)}
          />
        ))}
      </section>
    </>
  )
}

function JudgmentStatus({ judgment }: { judgment: Record<string, unknown> }) {
  const status = judgment?.status as string | undefined
  const tone = status === 'accepted' ? 'ok' : status === 'partial' ? 'warn' : status === 'blocked' ? 'live' : 'muted'
  const acceptedTerms = (judgment?.accepted_terms as number) ?? 0
  const acceptedAliases = (judgment?.accepted_aliases as number) ?? 0
  const blockedTerms = (judgment?.blocked_terms as number) ?? 0
  const blockedAliases = (judgment?.blocked_aliases as number) ?? 0
  return (
    <div className="flex items-center gap-3 flex-wrap">
      {status && <Badge value={status} tone={tone} />}
      {(acceptedTerms + acceptedAliases) > 0 && (
        <span className="text-[11px] text-ok tabular-nums">+{acceptedTerms}t +{acceptedAliases}a accepted</span>
      )}
      {(blockedTerms + blockedAliases) > 0 && (
        <span className="text-[11px] text-live tabular-nums">{blockedTerms}t {blockedAliases}a blocked</span>
      )}
    </div>
  )
}

function Learning({ events, loading }: { events: RuntimeLearningEventSummary[]; loading: boolean }) {
  return (
    <section className="card !p-0 overflow-hidden">
      <div className="px-5 py-3 border-b border-border flex items-center justify-between">
        <h2 className="text-[13px] font-semibold">Learning events</h2>
        <span className="text-[11px] text-fg-4">newest first · hashes and metadata only</span>
      </div>
      {loading ? (
        <div className="px-5 py-12 text-center text-[13px] text-fg-4">Loading learning events…</div>
      ) : events.length === 0 ? (
        <div className="px-5 py-12 text-center text-[13px] text-fg-4">No learning events yet.</div>
      ) : events.map(event => (
        <div key={event.id} className="border-b border-border-light last:border-b-0 px-5 py-4">
          <div className="flex items-start justify-between gap-4">
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-2 mb-1 flex-wrap">
                <span className="text-[12px] font-semibold text-fg truncate">{event.event_type}</span>
                {event.classification && <Chip>{event.classification}</Chip>}
                <JudgmentStatus judgment={event.server_judgment} />
              </div>
              <div className="text-[11px] text-fg-4">{event.account_email} · {timeAgo(event.created_at)}</div>
            </div>
            <div className="text-[11px] text-fg-4 tabular-nums whitespace-nowrap">{formatIstDateTime(event.created_at)}</div>
          </div>

          <div className="grid grid-cols-4 gap-3 mt-3">
            <Info label="Input" value={shortHash(event.input_hash)} mono />
            <Info label="Output" value={shortHash(event.output_hash)} mono />
            <Info label="Corrected" value={shortHash(event.corrected_hash)} mono />
            <Info label="Run" value={event.run_id ? shortHash(event.run_id) : '—'} mono />
          </div>

          <div className="grid grid-cols-2 gap-4 mt-4">
            <Panel title="Payload">
              <pre className="text-[11px] text-fg-4 whitespace-pre-wrap font-mono max-h-[180px] overflow-auto">{safeJson(event.payload_json)}</pre>
            </Panel>
            <Panel title="Server judgment">
              <pre className="text-[11px] text-fg-4 whitespace-pre-wrap font-mono max-h-[180px] overflow-auto">{safeJson(event.server_judgment)}</pre>
            </Panel>
          </div>
        </div>
      ))}
    </section>
  )
}

function LogRow({ run, open, detail, onToggle }: { run: RuntimeRunSummary; open: boolean; detail?: RuntimeRunDetail; onToggle: () => void }) {
  const latency = totalLatency(run)
  const failed = run.status !== 'completed'
  return (
    <div className="border-b border-border-light last:border-b-0">
      <button className="w-full flex items-center gap-3 px-4 py-3 hover:bg-surface-4/30 text-left" onClick={onToggle}>
        <ChevronRight size={14} className={`transition-transform duration-150 text-fg-4 ${open ? 'rotate-90' : ''}`} />
        <div className="w-[145px] min-w-0 shrink-0">
          <div className="text-[12px] font-medium leading-tight truncate">{run.account_email}</div>
          <div className="text-[10px] text-fg-4 leading-tight">{timeAgo(run.created_at)}</div>
        </div>
        <div className="flex items-center gap-1.5 shrink-0">
          <Chip>{runMode(run)}</Chip>
          <Chip>{run.source.replace('desktop_', '').slice(0, 18)}</Chip>
        </div>
        <div className="flex-1 min-w-0 hidden md:block">
          <div className="text-[12px] text-fg-3 truncate">{modelLabel(run, detail)}</div>
          <div className="text-[10px] text-fg-5 font-mono truncate">client {run.client_run_id || '—'}</div>
        </div>
        <Badge value={run.status} tone={failed ? 'live' : 'ok'} />
        <span className={`text-[12px] tabular-nums w-[72px] text-right shrink-0 ${failed ? 'text-live' : 'text-fg-3'}`}>{latency ? `${latency} ms` : '—'}</span>
      </button>

      {open && (
        <div className="px-5 py-4 bg-surface-2 border-t border-border-light pl-[52px]">
          <div className="grid grid-cols-3 gap-4 mb-4">
            <Info label="Input hash" value={shortHash(run.input_hash)} mono />
            <Info label="Output hash" value={shortHash(run.output_hash)} mono />
            <Info label="Created" value={formatIstDateTime(run.created_at)} />
            <Info label="Run id" value={run.id} mono />
            <Info label="Account" value={run.account_email} />
            <Info label="Account id" value={run.account_id} mono />
            <Info label="Platform" value={[run.platform, run.app_version].filter(Boolean).join(' · ') || '—'} />
            <Info label="Error" value={run.error_kind || '—'} />
            <Info label="Mode" value={run.mode} />
          </div>

          {detail ? (
            <>
              <div className="text-[10px] font-semibold text-fg-4 uppercase tracking-wider mb-2">Stage timeline</div>
              <StageTimeline stages={detail.stages} />
              <div className="grid grid-cols-2 gap-4 mt-4">
                <Panel title="Provider usage">
                  {detail.provider_usage.length === 0 ? <EmptyLine /> : detail.provider_usage.map(p => (
                    <div key={p.id} className="py-2 border-b border-border-light last:border-b-0 text-[12px]">
                      <div className="flex items-center justify-between gap-2">
                        <span className="font-medium text-fg">{p.provider}</span>
                        <Badge value={p.status} tone={p.status === 'ok' || p.status === 'connected' ? 'ok' : 'live'} />
                      </div>
                      <div className="text-fg-4 mt-1">{p.model || '—'} · {p.credential_scope} · {p.total_ms ?? p.request_ms ?? 0} ms</div>
                      {p.error_kind && <div className="text-live mt-1">{p.error_kind}</div>}
                    </div>
                  ))}
                </Panel>
                <Panel title="Metadata">
                  <pre className="text-[11px] text-fg-4 whitespace-pre-wrap font-mono max-h-[220px] overflow-auto">{safeJson(run.metadata_json)}</pre>
                </Panel>
              </div>
            </>
          ) : (
            <div className="flex items-center gap-2 text-[12px] text-fg-4"><Clock size={13} /> Loading detail…</div>
          )}
        </div>
      )}
    </div>
  )
}

function Info({ label, value, mono = false }: { label: string; value: string; mono?: boolean }) {
  return (
    <div>
      <div className="text-[10px] font-semibold text-fg-5 uppercase tracking-wider mb-1">{label}</div>
      <div className={`text-[12px] text-fg-3 truncate ${mono ? 'font-mono' : ''}`}>{value}</div>
    </div>
  )
}

function Panel({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div className="rounded-lg border border-border bg-surface/40 p-3">
      <div className="text-[10px] font-semibold text-fg-5 uppercase tracking-wider mb-2">{title}</div>
      {children}
    </div>
  )
}

function EmptyLine() {
  return <div className="text-[12px] text-fg-4">No rows recorded.</div>
}

function Chip({ children }: { children: React.ReactNode }) {
  return <span className="text-[10px] font-semibold px-1.5 py-0.5 rounded uppercase tracking-wide bg-surface-4 text-fg-3">{children}</span>
}
