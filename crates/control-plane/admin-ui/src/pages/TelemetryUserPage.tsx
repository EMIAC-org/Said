import { Fragment, useEffect, useState } from 'react'
import { useNavigate, useParams, useSearchParams } from 'react-router'
import { ChevronRight } from 'lucide-react'
import { apiJson } from '../api'
import { useAuth } from '../hooks/useAuth'
import { Avatar } from '../components/Avatar'
import { RunDetailPanel } from '../components/telemetry/RunDetailPanel'
import { DictationInspector } from '../components/telemetry/DictationInspector'
import { pct, ms, speechLabel, usd } from '../components/telemetry/format'
import { Loading, ErrorBox } from '../components/States'
import type {
  TelemetryRun,
  TelemetryUserMemory,
  TelemetryUserProfile,
} from '../types'

function AuthBadge({ source, lark }: { source: string; lark: boolean }) {
  const isLark = lark || source === 'lark'
  return (
    <span
      className="text-[10px] font-semibold px-2 py-0.5 rounded-full"
      style={{
        background: isLark ? 'hsl(145 60% 16%)' : 'hsl(210 60% 16%)',
        color: isLark ? 'hsl(145 70% 65%)' : 'hsl(210 70% 68%)',
      }}
    >
      {isLark ? 'Lark' : 'Email only'}
    </span>
  )
}

function SectionLabel({ children }: { children: React.ReactNode }) {
  return (
    <div className="text-[10px] font-semibold text-fg-4 uppercase tracking-wider mb-3">
      {children}
    </div>
  )
}

function CountGrid({ items }: { items: [string, number][] }) {
  return (
    <div className="grid grid-cols-3 gap-2 gap-x-4 text-[12px]">
      {items.map(([k, v]) => (
        <div key={k}>
          <div className="text-fg-4 text-[11px]">{k}</div>
          <div className="text-fg font-semibold tabular-nums mt-0.5">{v}</div>
        </div>
      ))}
    </div>
  )
}

function PromptProfileCard({ memory }: { memory: TelemetryUserMemory }) {
  const [showServerProfile, setShowServerProfile] = useState(false)
  const latest = memory.prompt_profile_latest
  const server = memory.server_learned_profile
  const serverDiffers =
    latest &&
    server &&
    server.profile_markdown.trim() &&
    latest.profile_markdown.trim() !== server.profile_markdown.trim()

  const copyMarkdown = async () => {
    if (!latest?.profile_markdown) return
    try {
      await navigator.clipboard.writeText(latest.profile_markdown)
    } catch {
      /* ignore */
    }
  }

  return (
    <div className="card p-4 mb-4">
      <div className="flex items-start justify-between gap-3 mb-3">
        <div>
          <SectionLabel>Prompt profile (last used in polish)</SectionLabel>
          <p className="text-[11px] text-fg-4 mt-1">
            Sanitized markdown injected into the voice polish system prompt on the most recent server
            runtime session.
          </p>
        </div>
        {latest?.profile_markdown ? (
          <button
            type="button"
            onClick={copyMarkdown}
            className="text-[11px] font-medium px-2.5 py-1 rounded-lg border border-border text-fg-3 hover:text-fg shrink-0"
          >
            Copy
          </button>
        ) : null}
      </div>
      {!latest || latest.profile_source === 'none' || !latest.profile_markdown.trim() ? (
        <p className="text-[12px] text-fg-4">
          No prompt profile recorded yet. Run a server-runtime dictation with a signed-in desktop
          client that sends <span className="font-mono">client_profile_markdown</span>.
        </p>
      ) : (
        <>
          <div className="flex flex-wrap gap-1.5 mb-3">
            <span className="text-[10px] px-2 py-0.5 rounded bg-info-bg text-info font-mono">
              {latest.profile_source}
            </span>
            <span className="text-[10px] px-2 py-0.5 rounded bg-surface-4 text-fg-3 tabular-nums">
              {latest.profile_chars} chars
            </span>
            {latest.client_profile_version != null ? (
              <span className="text-[10px] px-2 py-0.5 rounded bg-surface-4 text-fg-3 tabular-nums">
                v{latest.client_profile_version}
              </span>
            ) : null}
            <span className="text-[10px] px-2 py-0.5 rounded bg-surface-4 text-fg-3 font-mono truncate max-w-[12rem]">
              {latest.profile_hash.slice(0, 12)}…
            </span>
            <span className="text-[10px] px-2 py-0.5 rounded bg-surface-4 text-fg-3">
              {new Date(latest.updated_at).toLocaleString()}
            </span>
            {latest.last_run_id ? (
              <span className="text-[10px] px-2 py-0.5 rounded bg-surface-4 text-fg-3 font-mono">
                run {latest.last_run_id.slice(0, 8)}…
              </span>
            ) : null}
          </div>
          <pre className="text-[11px] leading-relaxed whitespace-pre-wrap break-words font-mono bg-surface-3 rounded-lg p-3 border border-border-light max-h-[28rem] overflow-auto">
            {latest.profile_markdown}
          </pre>
        </>
      )}
      {serverDiffers ? (
        <div className="mt-4 pt-4 border-t border-border-light">
          <button
            type="button"
            onClick={() => setShowServerProfile(v => !v)}
            className="text-[11px] text-warn hover:underline bg-transparent border-0 cursor-pointer p-0"
          >
            {showServerProfile ? 'Hide' : 'Show'} server learned profile (differs from last prompt
            snapshot)
          </button>
          {showServerProfile ? (
            <div className="mt-2">
              <div className="flex flex-wrap gap-1.5 mb-2 text-[10px] text-fg-4">
                <span>status: {server.status}</span>
                <span>v{server.version}</span>
                <span>{new Date(server.updated_at).toLocaleString()}</span>
              </div>
              <pre className="text-[11px] leading-relaxed whitespace-pre-wrap break-words font-mono bg-surface-3 rounded-lg p-3 border border-border-light max-h-[20rem] overflow-auto">
                {server.profile_markdown.trim() || '—'}
              </pre>
            </div>
          ) : null}
        </div>
      ) : null}
    </div>
  )
}

export function TelemetryUserPage() {
  const { accountId } = useParams<{ accountId: string }>()
  const { org } = useAuth()
  const navigate = useNavigate()
  const [searchParams, setSearchParams] = useSearchParams()
  const days = Number(searchParams.get('days') || '30')
  const modeFilter = searchParams.get('mode') || 'all'
  const tabParam = searchParams.get('tab')
  const initialTab =
    tabParam === 'memory' ? 'memory' : tabParam === 'dictation' ? 'dictation' : 'telemetry'

  const [profile, setProfile] = useState<TelemetryUserProfile | null>(null)
  const [runs, setRuns] = useState<TelemetryRun[]>([])
  const [runsTotal, setRunsTotal] = useState(0)
  const [runsOffset, setRunsOffset] = useState(0)
  const [expandedRun, setExpandedRun] = useState<string | null>(null)
  const [innerTab, setInnerTab] = useState<'telemetry' | 'memory' | 'dictation'>(initialTab)
  const [memory, setMemory] = useState<TelemetryUserMemory | null>(null)
  const [memoryLoading, setMemoryLoading] = useState(false)
  const [dictationFocusKey, setDictationFocusKey] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [runsLoading, setRunsLoading] = useState(false)
  const [error, setError] = useState('')

  const orgId = org?.org?.id
  const limit = 25

  useEffect(() => {
    if (!orgId || !accountId) {
      setLoading(false)
      return
    }
    setLoading(true)
    apiJson<TelemetryUserProfile>(
      `/v1/orgs/${orgId}/telemetry/users/${accountId}?days=${days}`,
    )
      .then(setProfile)
      .catch(e => setError(e.message))
      .finally(() => setLoading(false))
  }, [orgId, accountId, days])

  useEffect(() => {
    if (!orgId || !accountId) return
    setRunsLoading(true)
    setRunsOffset(0)
    const params = new URLSearchParams({ days: String(days), limit: String(limit), offset: '0' })
    if (modeFilter && modeFilter !== 'all') params.set('mode', modeFilter)
    apiJson<{ runs: TelemetryRun[]; total: number }>(
      `/v1/orgs/${orgId}/telemetry/users/${accountId}/runs?${params}`,
    )
      .then(d => {
        setRuns(d.runs || [])
        setRunsTotal(d.total || 0)
        setRunsOffset((d.runs || []).length)
      })
      .catch(() => {
        setRuns([])
        setRunsTotal(0)
      })
      .finally(() => setRunsLoading(false))
  }, [orgId, accountId, days, modeFilter])

  useEffect(() => {
    if (!orgId || !accountId || innerTab !== 'memory') return
    setMemoryLoading(true)
    apiJson<TelemetryUserMemory>(`/v1/orgs/${orgId}/telemetry/users/${accountId}/memory`)
      .then(setMemory)
      .catch(() => setMemory(null))
      .finally(() => setMemoryLoading(false))
  }, [orgId, accountId, innerTab])

  const switchTab = (tab: 'telemetry' | 'memory' | 'dictation') => {
    setInnerTab(tab)
    const p = new URLSearchParams(searchParams)
    if (tab === 'telemetry') p.delete('tab')
    else p.set('tab', tab)
    setSearchParams(p)
  }

  const openDictation = (recordingId: string) => {
    setDictationFocusKey(recordingId)
    switchTab('dictation')
  }

  const loadMoreRuns = () => {
    if (!orgId || !accountId || runsLoading) return
    setRunsLoading(true)
    const params = new URLSearchParams({
      days: String(days),
      limit: String(limit),
      offset: String(runsOffset),
    })
    if (modeFilter && modeFilter !== 'all') params.set('mode', modeFilter)
    apiJson<{ runs: TelemetryRun[]; total: number }>(
      `/v1/orgs/${orgId}/telemetry/users/${accountId}/runs?${params}`,
    )
      .then(d => {
        setRuns(prev => [...prev, ...(d.runs || [])])
        setRunsOffset(prev => prev + (d.runs || []).length)
        setRunsTotal(d.total || 0)
      })
      .finally(() => setRunsLoading(false))
  }

  if (loading) return <Loading />
  if (error) return <ErrorBox title="Failed to load user telemetry" message={error} />
  if (!profile) return null

  const { member } = profile
  const name = member.lark_name || member.email
  const lat = profile.latency_ms
  const costRuns = Math.max(profile.costs.summary.runs, 1)
  const costTracking = `STT ${pct(profile.costs.summary.stt_costed_runs / costRuns)} · polish ${pct(profile.costs.summary.polish_costed_runs / costRuns)} tracked`

  return (
    <>
      <button
        type="button"
        onClick={() => navigate(`/telemetry/users?days=${days}`)}
        className="inline-flex items-center gap-1.5 text-[12px] text-fg-4 hover:text-fg-2 mb-4 bg-transparent border-0 cursor-pointer p-0"
      >
        ← Back to users
      </button>

      <div className="flex items-start justify-between gap-4 mb-5 flex-wrap">
        <div className="flex items-center gap-3.5">
          <Avatar name={name} size="lg" />
          <div>
            <h1 className="text-xl font-semibold tracking-tight">{name}</h1>
            <p className="text-[12px] text-fg-4">{member.email}</p>
            <div className="flex items-center gap-2 mt-2 flex-wrap">
              <AuthBadge source={member.auth_source} lark={member.lark_connected} />
              {member.lark_department && (
                <span className="text-[10px] font-semibold px-2 py-0.5 rounded-full bg-surface-4 text-fg-3">
                  {member.lark_department}
                </span>
              )}
              <span
                className={`text-[10px] font-semibold px-2 py-0.5 rounded-full ${
                  member.desktop_active ? 'bg-ok-bg text-ok' : 'bg-surface-4 text-fg-3'
                }`}
              >
                {member.desktop_active ? 'Desktop online' : 'Desktop offline'}
              </span>
              <span className="font-mono text-[10px] text-fg-4">
                {member.account_id.substring(0, 8)}…
              </span>
            </div>
          </div>
        </div>
        <select
          value={days}
          onChange={e => {
            const p = new URLSearchParams(searchParams)
            p.set('days', e.target.value)
            setSearchParams(p)
          }}
          className="text-[12px] px-2.5 py-1.5 rounded-lg border border-border bg-surface-2 text-fg"
        >
          <option value="7">Last 7 days</option>
          <option value="30">Last 30 days</option>
          <option value="90">Last 90 days</option>
        </select>
      </div>

      <div className="flex gap-2 mb-4">
        {(
          [
            ['telemetry', 'Telemetry'],
            ['memory', 'Vocab & memory'],
            ['dictation', 'Dictation'],
          ] as const
        ).map(([tab, label]) => (
          <button
            key={tab}
            type="button"
            onClick={() => switchTab(tab)}
            className={`text-[12px] font-medium px-3 py-1.5 rounded-lg border transition-colors ${
              innerTab === tab
                ? 'border-accent bg-accent-light text-accent'
                : 'border-border bg-surface-2 text-fg-3 hover:text-fg'
            }`}
          >
            {label}
          </button>
        ))}
      </div>

      {innerTab === 'dictation' ? (
        orgId && accountId ? (
          <DictationInspector
            orgId={orgId}
            accountId={accountId}
            days={days}
            focusKey={dictationFocusKey}
          />
        ) : null
      ) : innerTab === 'memory' ? (
        memoryLoading ? (
          <Loading />
        ) : !memory ? (
          <div className="card p-6 text-[13px] text-fg-3">No personal memory on control plane yet.</div>
        ) : (
          <>
            {memory.hygiene.pending_review && (
              <div className="card p-4 mb-4 border-warn/30 bg-warn-bg/40">
                <div className="text-[12px] font-medium text-warn">Hygiene review pending</div>
                <p className="text-[11px] text-fg-3 mt-1">
                  Memory was updated; DeepSeek hygiene runs after a 30-minute quiet period.
                  {memory.hygiene.last_hygiene_at &&
                    ` Last run: ${new Date(memory.hygiene.last_hygiene_at).toLocaleString()}.`}
                </p>
              </div>
            )}
            <PromptProfileCard memory={memory} />
            <div className="card p-4 mb-4">
              <SectionLabel>Memory KPIs</SectionLabel>
              <CountGrid
                items={[
                  ['Aliases', memory.aliases.length],
                  ['Vocab terms', memory.vocab_terms.length],
                  ['Edit policies', memory.edit_policies.length],
                  ['Hygiene pending', memory.hygiene.pending_review ? 1 : 0],
                  ['Audit entries', memory.audit_log.length],
                ]}
              />
            </div>
            <div className="grid grid-cols-2 gap-4 mb-4">
              <div className="card !p-0 overflow-hidden">
                <div className="px-5 py-3 border-b border-border">
                  <SectionLabel>Vocab terms</SectionLabel>
                </div>
                <table className="w-full">
                  <thead>
                    <tr>
                      {['Term', 'Type', 'Weight', 'Hits', 'Status'].map(h => (
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
                    {memory.vocab_terms.map((v, i) => (
                      <tr key={i}>
                        <td className="text-[12px] px-4 py-2 border-b border-border-light font-mono">{v.term}</td>
                        <td className="text-[11px] px-4 py-2 border-b border-border-light">{v.term_type}</td>
                        <td className="text-[12px] tabular-nums px-4 py-2 border-b border-border-light">{v.weight}</td>
                        <td className="text-[12px] tabular-nums px-4 py-2 border-b border-border-light">{v.positive_count}</td>
                        <td className="text-[11px] px-4 py-2 border-b border-border-light">{v.status}</td>
                      </tr>
                    ))}
                    {!memory.vocab_terms.length && (
                      <tr>
                        <td colSpan={5} className="text-[12px] text-fg-4 px-4 py-3">No vocab terms.</td>
                      </tr>
                    )}
                  </tbody>
                </table>
              </div>
              <div className="card !p-0 overflow-hidden">
                <div className="px-5 py-3 border-b border-border">
                  <SectionLabel>Edit policies</SectionLabel>
                </div>
                <table className="w-full">
                  <thead>
                    <tr>
                      {['Variant', 'Correct', 'Type', '+', '−', 'Status'].map(h => (
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
                    {memory.edit_policies.map((p, i) => (
                      <tr key={i}>
                        <td className="text-[12px] px-4 py-2 border-b border-border-light font-mono">{p.variant_form}</td>
                        <td className="text-[12px] px-4 py-2 border-b border-border-light font-mono">{p.correct_form}</td>
                        <td className="text-[11px] px-4 py-2 border-b border-border-light">{p.edit_type}</td>
                        <td className="text-[12px] tabular-nums px-4 py-2 border-b border-border-light">{p.positive_count}</td>
                        <td className="text-[12px] tabular-nums px-4 py-2 border-b border-border-light">{p.negative_count}</td>
                        <td className="text-[11px] px-4 py-2 border-b border-border-light">{p.status}</td>
                      </tr>
                    ))}
                    {!memory.edit_policies.length && (
                      <tr>
                        <td colSpan={6} className="text-[12px] text-fg-4 px-4 py-3">No edit policies.</td>
                      </tr>
                    )}
                  </tbody>
                </table>
              </div>
            </div>
            <div className="grid grid-cols-2 gap-4 mb-4">
              <div className="card !p-0 overflow-hidden">
                <div className="px-5 py-3 border-b border-border">
                  <SectionLabel>STT aliases</SectionLabel>
                </div>
                <table className="w-full">
                  <thead>
                    <tr>
                      {['Heard', 'Correct', 'Safety', 'STT', 'Hits'].map(h => (
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
                    {memory.aliases.map((a, i) => (
                      <tr key={i}>
                        <td className="text-[12px] px-4 py-2 border-b border-border-light font-mono">{a.transcript_form}</td>
                        <td className="text-[12px] px-4 py-2 border-b border-border-light font-mono">{a.correct_form}</td>
                        <td className="text-[11px] px-4 py-2 border-b border-border-light">{a.safety_status}</td>
                        <td className="text-[11px] px-4 py-2 border-b border-border-light font-mono">{a.learned_speech_model || '—'}</td>
                        <td className="text-[12px] tabular-nums px-4 py-2 border-b border-border-light">{a.positive_count}</td>
                      </tr>
                    ))}
                    {!memory.aliases.length && (
                      <tr>
                        <td colSpan={5} className="text-[12px] text-fg-4 px-4 py-3">No aliases.</td>
                      </tr>
                    )}
                  </tbody>
                </table>
              </div>
              <div className="card !p-0 overflow-hidden">
                <div className="px-5 py-3 border-b border-border">
                  <SectionLabel>Hygiene audit</SectionLabel>
                </div>
                <table className="w-full">
                  <thead>
                    <tr>
                      {['When', 'Action', 'Heard', 'Correct', 'Reason'].map(h => (
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
                    {memory.audit_log.map((row, i) => (
                      <tr key={i}>
                        <td className="text-[11px] px-4 py-2 border-b border-border-light">{new Date(row.created_at).toLocaleString()}</td>
                        <td className="text-[11px] px-4 py-2 border-b border-border-light font-mono">{row.action}</td>
                        <td className="text-[11px] px-4 py-2 border-b border-border-light font-mono">{row.heard || '—'}</td>
                        <td className="text-[11px] px-4 py-2 border-b border-border-light font-mono">{row.correct || '—'}</td>
                        <td className="text-[11px] px-4 py-2 border-b border-border-light text-fg-3">{row.reason}</td>
                      </tr>
                    ))}
                    {!memory.audit_log.length && (
                      <tr>
                        <td colSpan={5} className="text-[12px] text-fg-4 px-4 py-3">No hygiene actions yet.</td>
                      </tr>
                    )}
                  </tbody>
                </table>
              </div>
            </div>
          </>
        )
      ) : (
        <>
      <div className="card !p-0 overflow-hidden mb-4">
        <div className="grid grid-cols-2 md:grid-cols-3 xl:grid-cols-6">
          {[
            ['Dictations', profile.summary.runs.toLocaleString(), `${profile.summary.audio_minutes} min`],
            ['Acceptance', pct(profile.quality.acceptance_rate), 'accepted as-is'],
            ['Heavy edits', pct(profile.quality.heavy_edit_rate), `${pct(profile.quality.edit_rate)} edited`],
            ['Latency p95', ms(lat.total_p95), `p50 ${ms(lat.total_p50)}`],
            ['Failures', profile.quality_counts.failures.toLocaleString(), `${pct(profile.quality.fallback_rate)} fallback`],
            ['Estimated cost', usd(profile.costs.summary.total_usd), costTracking],
          ].map(([label, value, sub]) => (
            <div key={label} className="px-4 py-4 border-b border-r border-border-light last:border-r-0">
              <div className="text-[10px] font-semibold text-fg-4 uppercase tracking-wider">{label}</div>
              <div className="text-[20px] font-semibold tabular-nums mt-1.5">{value}</div>
              <div className="text-[10px] text-fg-4 mt-0.5">{sub}</div>
            </div>
          ))}
        </div>
      </div>

      <div className="grid grid-cols-1 lg:grid-cols-2 gap-4 mb-4">
        <div className="card p-4">
          <SectionLabel>Speech recognition</SectionLabel>
          <div className="space-y-2 text-[12px]">
            {profile.costs.by_model.stt.map(row => (
              <div key={`${row.provider}:${row.model}`} className="flex justify-between gap-3">
                <div className="min-w-0">
                  <div className="text-fg-2 truncate" title={`${row.provider} · ${row.model}`}>
                    {speechLabel(`${row.provider}:${row.model}`)}
                  </div>
                  <div className="text-[10px] text-fg-4">
                    {row.runs} runs · {row.audio_minutes.toFixed(1)} min
                  </div>
                </div>
                <span className="tabular-nums shrink-0">{usd(row.cost_usd)}</span>
              </div>
            ))}
            {!profile.costs.by_model.stt.length && <div className="text-fg-4">No STT cost data.</div>}
          </div>
        </div>
        <div className="card p-4">
          <SectionLabel>Polish spend</SectionLabel>
          <div className="space-y-2 text-[12px]">
            {profile.costs.by_model.polish.map(row => (
              <div key={`${row.provider}:${row.model || 'unknown'}`} className="flex justify-between gap-3">
                <div className="min-w-0">
                  <div className="font-mono text-fg-2 truncate">{row.provider} · {row.model || 'unknown'}</div>
                  <div className="text-[10px] text-fg-4">
                    {row.attempts} attempts · {row.input_tokens.toLocaleString()} in / {row.output_tokens.toLocaleString()} out
                  </div>
                </div>
                <span className="tabular-nums shrink-0">{usd(row.cost_usd)}</span>
              </div>
            ))}
            {!profile.costs.by_model.polish.length && <div className="text-fg-4">No polish usage recorded.</div>}
          </div>
        </div>
      </div>

      <div className="card !p-0 overflow-hidden">
        <div className="px-5 py-4 border-b border-border flex items-center justify-between gap-3">
          <div>
            <SectionLabel>Recent runs</SectionLabel>
            <p className="text-[11px] text-fg-4">Click row to expand all fields</p>
          </div>
          <select
            value={modeFilter}
            onChange={e => {
              const p = new URLSearchParams(searchParams)
              if (e.target.value === 'all') p.delete('mode')
              else p.set('mode', e.target.value)
              setSearchParams(p)
            }}
            className="text-[12px] px-2.5 py-1.5 rounded-lg border border-border bg-surface-2 text-fg"
          >
            <option value="all">All modes</option>
            <option value="normal_voice">normal_voice</option>
            <option value="command">command</option>
            <option value="rewrite_selection">rewrite_selection</option>
          </select>
        </div>
        <div className="overflow-x-auto overscroll-x-contain">
        <table className="w-full min-w-[760px] table-fixed">
          <thead>
            <tr>
              <th className="w-10" />
              {[
                ['When', 'w-[20%]'],
                ['Context', 'w-[20%]'],
                ['STT', 'w-[20%]'],
                ['Cost', 'w-[12%]'],
                ['Latency', 'w-[12%]'],
                ['Result', 'w-[16%]'],
              ].map(
                ([h, width]) => (
                  <th
                    key={h}
                    className={`${width} text-[10px] font-medium text-fg-4 text-left px-3 py-3 border-b border-border uppercase tracking-wider`}
                  >
                    {h}
                  </th>
                ),
              )}
            </tr>
          </thead>
          <tbody>
            {runs.map(run => {
              const open = expandedRun === run.run_id
              const bucketClass =
                run.edit_bucket === 'none'
                  ? 'bg-ok-bg text-ok'
                  : run.edit_bucket === 'heavy' || run.edit_bucket === 'full_replace'
                    ? 'bg-live-bg text-live'
                    : 'bg-warn-bg text-warn'
              return (
                <Fragment key={run.run_id}>
                  <tr
                    className="cursor-pointer hover:bg-surface-4/30 transition-colors"
                    onClick={() => setExpandedRun(open ? null : run.run_id)}
                  >
                    <td className="px-3 py-2.5 border-b border-border-light text-fg-4">
                      <ChevronRight
                        size={14}
                        className={`transition-transform ${open ? 'rotate-90' : ''}`}
                      />
                    </td>
                    <td className="px-3 py-2.5 border-b border-border-light">
                      <div className="text-[11px]">{new Date(run.event_at).toLocaleString()}</div>
                      <div className="text-[9px] text-fg-4 font-mono mt-0.5">{run.run_id.slice(0, 8)}…</div>
                    </td>
                    <td className="px-3 py-2.5 border-b border-border-light">
                      <div className="text-[12px] truncate">{run.target_app || 'Unknown app'}</div>
                      <div className="text-[9px] text-fg-4 font-mono mt-0.5">{run.mode}</div>
                    </td>
                    <td
                      title={`${run.speech_provider || 'unknown'} · ${run.speech_model || 'unknown'}`}
                      className="px-3 py-2.5 border-b border-border-light"
                    >
                      <div className="text-[11px] truncate">
                        {speechLabel(`${run.speech_provider || ''}:${run.speech_model || ''}`)}
                      </div>
                      <div className="text-[9px] text-fg-4 mt-0.5">{run.speech_path || '—'}</div>
                    </td>
                    <td className="text-[11px] tabular-nums px-3 py-2.5 border-b border-border-light">
                      <div>{usd(run.total_cost_usd)}</div>
                      <div className="text-[9px] text-fg-4 mt-0.5">{run.cost_coverage}</div>
                    </td>
                    <td className="px-3 py-2.5 border-b border-border-light">
                      <div className="text-[12px] tabular-nums">{ms(run.total_ms)}</div>
                      <div className="text-[9px] text-fg-4 mt-0.5">
                        {run.audio_seconds != null ? `${run.audio_seconds}s audio` : '—'}
                      </div>
                    </td>
                    <td className="px-3 py-2.5 border-b border-border-light">
                      <div className="flex items-center gap-1.5 flex-wrap">
                        <span
                          className={`text-[10px] font-semibold px-2 py-0.5 rounded-full ${
                            run.success ? 'bg-ok-bg text-ok' : 'bg-live-bg text-live'
                          }`}
                        >
                          {run.success ? 'ok' : 'fail'}
                        </span>
                        <span className={`text-[10px] font-semibold px-2 py-0.5 rounded-full ${bucketClass}`}>
                          {run.edit_bucket}
                        </span>
                      </div>
                    </td>
                  </tr>
                  {open && (
                    <tr>
                      <td colSpan={7} className="p-0 border-b border-border-light">
                        <RunDetailPanel run={run} onOpenDictation={openDictation} />
                      </td>
                    </tr>
                  )}
                </Fragment>
              )
            })}
            {!runs.length && !runsLoading && (
              <tr>
                <td colSpan={7} className="text-[12px] text-fg-4 px-5 py-4">
                  No runs in this window.
                </td>
              </tr>
            )}
          </tbody>
        </table>
        </div>
        {runsOffset < runsTotal && (
          <div className="p-4 border-t border-border-light text-center">
            <button
              type="button"
              onClick={loadMoreRuns}
              disabled={runsLoading}
              className="text-[12px] font-medium px-4 py-2 rounded-lg border border-border text-fg-3 hover:text-fg hover:border-fg-5 transition-colors disabled:opacity-50"
            >
              {runsLoading ? 'Loading…' : `Load more (${runs.length} of ${runsTotal})`}
            </button>
          </div>
        )}
      </div>
        </>
      )}
    </>
  )
}
