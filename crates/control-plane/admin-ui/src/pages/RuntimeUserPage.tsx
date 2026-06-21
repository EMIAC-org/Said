import { useState } from 'react'
import { useParams, useNavigate } from 'react-router'
import {
  ArrowLeft,
  ChevronRight,
  Check,
  Pencil,
} from 'lucide-react'
import {
  getUser,
  getPolishes,
  getPersonalVocab,
  getSttAliases,
  getLearningEvents,
} from '../mock/runtime'
import type { RtPolish } from '../mock/runtime'
import {
  MetricCard,
  StageTimeline,
  Badge,
  Gauge,
  Sparkline,
} from '../components/charts'
import { Avatar } from '../components/Avatar'
import { ErrorBox } from '../components/States'
import { timeAgo, formatDate, formatTime, formatIstDateTime } from '../utils'

type Tab = 'polishes' | 'vocab' | 'aliases' | 'learning'

function Chip({ children }: { children: React.ReactNode }) {
  return (
    <span className="text-[10px] font-semibold px-2 py-0.5 rounded-md bg-surface-4 text-fg-3 uppercase tracking-wide">
      {children}
    </span>
  )
}

function modelLabel(modelUsed: string): { label: string; cls: string } {
  if (modelUsed.includes('maverick') || modelUsed.includes('scout')) {
    return { label: 'Smart', cls: 'text-info bg-info-bg' }
  }
  return { label: 'Fast', cls: 'bg-surface-4 text-fg-3' }
}

function PolishRow({ p, open, onToggle }: { p: RtPolish; open: boolean; onToggle: () => void }) {
  const ml = modelLabel(p.model_used)
  return (
    <>
      {/* Summary row */}
      <div
        className="px-5 py-3 border-b border-border-light hover:bg-surface-4/30 cursor-pointer flex items-center gap-3"
        onClick={onToggle}
      >
        {/* Chevron */}
        <ChevronRight
          size={14}
          className={`text-fg-4 shrink-0 transition-transform duration-150 ${open ? 'rotate-90' : ''}`}
        />

        {/* Time */}
        <div className="flex flex-col min-w-[70px]">
          <span className="text-[12px] text-fg-3">{timeAgo(p.created_at)}</span>
          <span className="text-[10px] text-fg-5">{formatTime(p.created_at)}</span>
        </div>

        {/* Mode chip */}
        <Chip>{p.mode === 'message_polish' ? 'message' : 'voice'}</Chip>

        {/* Source chip */}
        <Chip>{p.source === 'runtime_wav_probe' ? 'wav probe' : 'desktop'}</Chip>

        {/* Model chip */}
        <span className={`text-[10px] font-semibold px-2 py-0.5 rounded-md uppercase tracking-wide ${ml.cls}`}>
          {ml.label}
        </span>

        {/* Status badge */}
        <Badge value={p.status} />

        {/* Spacer */}
        <div className="flex-1" />

        {/* Accepted / edited indicator */}
        <div className="flex items-center gap-1 shrink-0">
          {p.accepted ? (
            <span title="Accepted without edits">
              <Check size={13} className="text-ok" />
            </span>
          ) : (
            <span title="Edited after polish">
              <Pencil size={12} className="text-warn" />
            </span>
          )}
        </div>

        {/* Latency */}
        <span
          className={`text-[12px] tabular-nums shrink-0 ${p.status === 'failed' ? 'text-live' : 'text-fg-3'}`}
        >
          {p.latency.total} ms
        </span>

        {/* Words */}
        <span className="text-[11px] text-fg-4 tabular-nums shrink-0 min-w-[40px] text-right">
          {p.words}w
        </span>
      </div>

      {/* Expanded detail */}
      {open && (
        <div className="px-5 py-4 bg-surface-2 border-b border-border-light">
          {/* Transcript */}
          <div className="mb-3">
            <div className="text-[10px] font-semibold text-fg-5 uppercase tracking-wider mb-1">Transcript</div>
            <div className="text-[13px] font-mono text-fg-3 leading-relaxed">{p.transcript}</div>
          </div>

          {/* Polished output */}
          <div className="mb-3">
            <div className="text-[10px] font-semibold text-fg-5 uppercase tracking-wider mb-1">Polished output</div>
            <div className="text-[13px] text-fg leading-relaxed">{p.output || '— (failed)'}</div>
          </div>

          {/* Meta row */}
          <div className="flex flex-wrap gap-x-4 gap-y-1 mb-4 text-[11px] text-fg-4">
            <span>STT: <span className="text-fg-3">{p.provider.stt}</span></span>
            <span>LLM: <span className="text-fg-3">{p.provider.llm}</span></span>
            <span className="tabular-nums">
              STT {p.latency.stt} ms · Polish {p.latency.polish} ms · Total {p.latency.total} ms
            </span>
            <span>{formatIstDateTime(p.created_at)}</span>
          </div>

          {/* Stage timeline */}
          <div className="text-[11px] font-semibold text-fg-4 uppercase tracking-wider mb-2">Stage timeline</div>
          <StageTimeline stages={p.stages} />
        </div>
      )}
    </>
  )
}

export function RuntimeUserPage() {
  const { id } = useParams()
  const navigate = useNavigate()

  const user = id ? getUser(id) : undefined

  const [tab, setTab] = useState<Tab>('polishes')
  const [open, setOpen] = useState<Set<string>>(new Set())

  // Fetch data regardless of early-return (hooks must be unconditional)
  const polishes = id ? getPolishes(id) : []
  const vocab = id ? getPersonalVocab(id) : []
  const aliases = id ? getSttAliases(id) : []
  const events = id ? getLearningEvents(id) : []

  function togglePolish(pid: string) {
    setOpen(prev => {
      const next = new Set(prev)
      if (next.has(pid)) next.delete(pid)
      else next.add(pid)
      return next
    })
  }

  // ── Not found ───────────────────────────────────────────────────────────────
  if (!user) {
    return (
      <>
        <div className="mb-4">
          <button
            className="flex items-center gap-1.5 text-[12px] text-fg-4 hover:text-fg-3"
            onClick={() => navigate('/runtime')}
          >
            <ArrowLeft size={13} /> Back to Runtime
          </button>
        </div>
        <ErrorBox title="User not found" message="This runtime user does not exist." />
      </>
    )
  }

  const tabs: [Tab, string][] = [
    ['polishes', 'Polishes'],
    ['vocab', `Personal vocab (${vocab.length})`],
    ['aliases', `STT aliases (${aliases.length})`],
    ['learning', 'Learning'],
  ]

  const pct = Math.round(user.acceptance_rate * 100)

  // Learning event dot/tone helpers
  function eventDotColor(type: string): string {
    if (type === 'accepted_edit' || type === 'learned') return 'bg-ok'
    if (type === 'rejected_edit') return 'bg-live'
    return 'bg-warn' // corrected
  }
  function eventTone(type: string): 'ok' | 'warn' | 'live' | 'muted' {
    if (type === 'learned' || type === 'accepted_edit') return 'ok'
    if (type === 'rejected_edit') return 'live'
    if (type === 'corrected') return 'warn'
    return 'muted'
  }

  return (
    <>
      {/* Back link */}
      <div className="mb-4">
        <button
          className="flex items-center gap-1.5 text-[12px] text-fg-4 hover:text-fg-3 transition-colors"
          onClick={() => navigate('/runtime')}
        >
          <ArrowLeft size={13} /> Back to Runtime
        </button>
      </div>

      {/* ── User header card ─────────────────────────────────────────────────── */}
      <div className="card mb-4">
        <div className="flex items-start gap-4">
          <Avatar name={user.name} size="xl" />
          <div className="flex-1 min-w-0">
            <div className="text-lg font-semibold tracking-tight">{user.name}</div>
            <div className="text-[12px] text-fg-4 mb-2">{user.email}</div>
            <div className="flex flex-wrap items-center gap-2">
              <Badge value={user.role} tone={user.role === 'admin' ? 'info' : 'muted'} />
              <Chip>{user.department}</Chip>
              <Chip>{user.platform}</Chip>
              <Chip>v{user.app_version}</Chip>
              <span className="text-[11px] text-fg-4">Active {timeAgo(user.last_active)}</span>
            </div>
          </div>
        </div>
      </div>

      {/* ── KPI row ──────────────────────────────────────────────────────────── */}
      <div className="grid grid-cols-4 gap-4 mb-4">
        <MetricCard
          label="Polishes (7d)"
          value={user.polish_count_7d}
          sub={<Sparkline points={user.daily} />}
        />
        <MetricCard
          label="Acceptance"
          value={`${pct}%`}
        />
        <MetricCard
          label="Avg latency"
          value={`${user.avg_total_ms} ms`}
        />
        <MetricCard
          label="Words (7d)"
          value={user.words_7d.toLocaleString()}
        />
      </div>

      {/* ── Tab bar ──────────────────────────────────────────────────────────── */}
      <div className="mb-4 flex gap-1.5">
        {tabs.map(([id, label]) => (
          <button
            key={id}
            onClick={() => setTab(id)}
            className={`h-8 rounded-lg px-3 text-[12px] font-medium ${
              tab === id ? 'bg-surface-4 text-fg' : 'text-fg-4 hover:text-fg hover:bg-surface-4/40'
            }`}
          >
            {label}
          </button>
        ))}
      </div>

      {/* ── POLISHES tab ─────────────────────────────────────────────────────── */}
      {tab === 'polishes' && (
        <section className="card !p-0 overflow-hidden">
          <div className="px-5 py-3 border-b border-border flex items-center justify-between">
            <h2 className="text-[13px] font-semibold">Recent Polishes</h2>
            <span className="text-[11px] text-fg-4">showing {polishes.length} polishes</span>
          </div>
          {polishes.length === 0 ? (
            <div className="px-5 py-10 text-center text-fg-3">
              <p className="text-[14px] font-semibold text-fg mb-1">No polishes yet</p>
              <p className="text-[12px]">No polish sessions found for this user.</p>
            </div>
          ) : (
            <div>
              {polishes.map(p => (
                <PolishRow
                  key={p.id}
                  p={p}
                  open={open.has(p.id)}
                  onToggle={() => togglePolish(p.id)}
                />
              ))}
            </div>
          )}
        </section>
      )}

      {/* ── VOCAB tab ────────────────────────────────────────────────────────── */}
      {tab === 'vocab' && (
        <section className="card !p-0 overflow-hidden">
          <div className="px-5 py-3 border-b border-border flex items-center justify-between">
            <h2 className="text-[13px] font-semibold">Personal Vocabulary</h2>
            <span className="text-[11px] text-fg-4">{vocab.length} terms</span>
          </div>
          {vocab.length === 0 ? (
            <div className="px-5 py-10 text-center text-fg-3">
              <p className="text-[14px] font-semibold text-fg mb-1">No personal vocab</p>
              <p className="text-[12px]">User has no learned vocabulary terms yet.</p>
            </div>
          ) : (
            <table className="w-full">
              <thead>
                <tr>
                  {['Term', 'Type', 'Source', 'Weight', '+ / −', 'Status', 'Last seen'].map(h => (
                    <th
                      key={h}
                      className="text-[10px] font-medium text-fg-4 text-left px-5 py-3 border-b border-border uppercase tracking-wider"
                    >
                      {h}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {vocab.map(v => (
                  <tr key={v.id} className="hover:bg-surface-4/30">
                    <td className="px-5 py-3 border-b border-border-light text-[13px] font-medium">{v.term}</td>
                    <td className="px-5 py-3 border-b border-border-light text-[12px] text-fg-3">{v.term_type}</td>
                    <td className="px-5 py-3 border-b border-border-light text-[12px] text-fg-4">{v.source}</td>
                    <td className="px-5 py-3 border-b border-border-light text-[12px] tabular-nums">{v.weight}</td>
                    <td className="px-5 py-3 border-b border-border-light text-[12px] tabular-nums">
                      <span className="text-ok">{v.positive_count}</span>
                      <span className="text-fg-4"> / </span>
                      <span className={v.negative_count > 0 ? 'text-live' : 'text-fg-4'}>
                        {v.negative_count}
                      </span>
                    </td>
                    <td className="px-5 py-3 border-b border-border-light">
                      <Badge value={v.status} />
                    </td>
                    <td className="px-5 py-3 border-b border-border-light text-[11px] text-fg-4">
                      {timeAgo(v.last_seen_at)}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </section>
      )}

      {/* ── ALIASES tab ──────────────────────────────────────────────────────── */}
      {tab === 'aliases' && (
        <section className="card !p-0 overflow-hidden">
          <div className="px-5 py-3 border-b border-border flex items-center justify-between">
            <h2 className="text-[13px] font-semibold">STT Aliases</h2>
            <span className="text-[11px] text-fg-4">{aliases.length} aliases</span>
          </div>
          {aliases.length === 0 ? (
            <div className="px-5 py-10 text-center text-fg-3">
              <p className="text-[14px] font-semibold text-fg mb-1">No STT aliases</p>
              <p className="text-[12px]">User has no learned STT correction aliases yet.</p>
            </div>
          ) : (
            <table className="w-full">
              <thead>
                <tr>
                  {['Heard', '→', 'Correct', 'Weight', '+ / −', 'Safety', 'Status', 'Last seen'].map(h => (
                    <th
                      key={h}
                      className="text-[10px] font-medium text-fg-4 text-left px-5 py-3 border-b border-border uppercase tracking-wider"
                    >
                      {h}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {aliases.map(a => (
                  <tr key={a.id} className="hover:bg-surface-4/30">
                    <td className="px-5 py-3 border-b border-border-light text-[13px] font-mono text-fg-3">
                      {a.transcript_form}
                    </td>
                    <td className="px-5 py-3 border-b border-border-light text-[12px] text-fg-5">→</td>
                    <td className="px-5 py-3 border-b border-border-light text-[13px] font-medium">
                      {a.correct_form}
                    </td>
                    <td className="px-5 py-3 border-b border-border-light text-[12px] tabular-nums">
                      {a.weight}
                    </td>
                    <td className="px-5 py-3 border-b border-border-light text-[12px] tabular-nums">
                      <span className="text-ok">{a.positive_count}</span>
                      <span className="text-fg-4"> / </span>
                      <span className={a.negative_count > 0 ? 'text-live' : 'text-fg-4'}>
                        {a.negative_count}
                      </span>
                    </td>
                    <td className="px-5 py-3 border-b border-border-light">
                      <Badge value={a.safety_status} />
                    </td>
                    <td className="px-5 py-3 border-b border-border-light">
                      <Badge value={a.status} />
                    </td>
                    <td className="px-5 py-3 border-b border-border-light text-[11px] text-fg-4">
                      {timeAgo(a.last_seen_at)}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </section>
      )}

      {/* ── LEARNING tab ─────────────────────────────────────────────────────── */}
      {tab === 'learning' && (
        <>
          {/* Summary card */}
          <div className="card mb-4 flex items-center gap-8">
            <Gauge value={user.acceptance_rate} label="acceptance" />
            <div>
              <p className="text-[14px] text-fg-2 leading-relaxed">
                <span className="font-semibold text-fg">{user.name.split(' ')[0]}</span> kept{' '}
                <span className="font-semibold text-accent">{pct}%</span> of polishes unedited over
                the last 7 days.
              </p>
              <p className="text-[12px] text-fg-4 mt-1">
                {user.polish_count_7d} sessions &middot; {events.length} learning events recorded
              </p>
            </div>
          </div>

          {/* Events timeline */}
          <section className="card !p-0 overflow-hidden">
            <div className="px-5 py-3 border-b border-border flex items-center justify-between">
              <h2 className="text-[13px] font-semibold">Learning Events</h2>
              <span className="text-[11px] text-fg-4">{events.length} events</span>
            </div>
            {events.length === 0 ? (
              <div className="px-5 py-10 text-center text-fg-3">
                <p className="text-[14px] font-semibold text-fg mb-1">No learning events</p>
                <p className="text-[12px]">No learning events recorded for this user yet.</p>
              </div>
            ) : (
              <div className="divide-y divide-border-light">
                {events.map(ev => (
                  <div key={ev.id} className="flex items-center gap-3 px-5 py-3 hover:bg-surface-4/30">
                    {/* Colored dot */}
                    <span
                      className={`w-2 h-2 rounded-full shrink-0 ${eventDotColor(ev.event_type)}`}
                    />
                    {/* Event type badge */}
                    <Badge value={ev.event_type} tone={eventTone(ev.event_type)} />
                    {/* Classification chip */}
                    <span className="text-[10px] font-mono text-fg-4 bg-surface-4 px-2 py-0.5 rounded">
                      {ev.classification}
                    </span>
                    {/* Detail */}
                    <span className="text-[12px] text-fg-3 flex-1 truncate">{ev.detail}</span>
                    {/* Time */}
                    <span className="text-[11px] text-fg-5 shrink-0">{timeAgo(ev.created_at)}</span>
                  </div>
                ))}
              </div>
            )}
          </section>
        </>
      )}
    </>
  )
}
