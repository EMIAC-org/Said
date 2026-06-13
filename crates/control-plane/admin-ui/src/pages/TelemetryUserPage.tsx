import { Fragment, useEffect, useState } from 'react'
import { useNavigate, useParams, useSearchParams } from 'react-router'
import { ChevronRight } from 'lucide-react'
import { apiJson } from '../api'
import { useAuth } from '../hooks/useAuth'
import { Avatar } from '../components/Avatar'
import { TelemetryStatCard } from '../components/telemetry/TelemetryStatCard'
import { RunDetailPanel } from '../components/telemetry/RunDetailPanel'
import { pct, ms } from '../components/telemetry/format'
import { Loading, ErrorBox } from '../components/States'
import type { TelemetryRun, TelemetryUserMemory, TelemetryUserProfile } from '../types'

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

export function TelemetryUserPage() {
  const { accountId } = useParams<{ accountId: string }>()
  const { org } = useAuth()
  const navigate = useNavigate()
  const [searchParams, setSearchParams] = useSearchParams()
  const days = Number(searchParams.get('days') || '30')
  const modeFilter = searchParams.get('mode') || 'all'

  const [profile, setProfile] = useState<TelemetryUserProfile | null>(null)
  const [runs, setRuns] = useState<TelemetryRun[]>([])
  const [runsTotal, setRunsTotal] = useState(0)
  const [runsOffset, setRunsOffset] = useState(0)
  const [expandedRun, setExpandedRun] = useState<string | null>(null)
  const [innerTab, setInnerTab] = useState<'telemetry' | 'memory'>('telemetry')
  const [memory, setMemory] = useState<TelemetryUserMemory | null>(null)
  const [memoryLoading, setMemoryLoading] = useState(false)
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

  const flagEntries = Object.entries(profile.content_flags || {}).filter(([, v]) => v > 0)

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
        {(['telemetry', 'memory'] as const).map(tab => (
          <button
            key={tab}
            type="button"
            onClick={() => setInnerTab(tab)}
            className={`text-[12px] font-medium px-3 py-1.5 rounded-lg border transition-colors ${
              innerTab === tab
                ? 'border-accent bg-accent-light text-accent'
                : 'border-border bg-surface-2 text-fg-3 hover:text-fg'
            }`}
          >
            {tab === 'telemetry' ? 'Telemetry' : 'Vocab & memory'}
          </button>
        ))}
      </div>

      {innerTab === 'memory' ? (
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
                        <td className="text-[11px] px-4 py-2 border-b border-border-light font-mono">{a.learned_stt_provider || '—'}</td>
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
      <div className="grid grid-cols-4 gap-3 mb-4">
        <TelemetryStatCard label="Runs" value={String(profile.summary.runs)} sub="completed" />
        <TelemetryStatCard
          label="Audio"
          value={String(profile.summary.audio_minutes)}
          sub="minutes dictated"
        />
        <TelemetryStatCard
          label="Words"
          value={profile.summary.word_count.toLocaleString()}
          sub={`chars ${profile.summary.char_count.toLocaleString()}`}
        />
        <TelemetryStatCard
          label="Acceptance"
          value={pct(profile.quality.acceptance_rate)}
          sub="accepted as-is"
        />
      </div>

      <div className="grid grid-cols-3 gap-3 mb-4">
        <TelemetryStatCard
          label="Edit rate"
          value={pct(profile.quality.edit_rate)}
          sub={`heavy ${pct(profile.quality.heavy_edit_rate)}`}
        />
        <TelemetryStatCard
          label="Fallback rate"
          value={pct(profile.quality.fallback_rate)}
          sub="clipboard or HTTP STT"
        />
        <TelemetryStatCard
          label="Latency p50 / p95"
          value={`${ms(lat.total_p50)} / ${ms(lat.total_p95)}`}
          sub="total pipeline"
        />
      </div>

      <div className="grid grid-cols-2 gap-4 mb-4">
        <div className="card p-4">
          <SectionLabel>Quality breakdown</SectionLabel>
          <CountGrid
            items={[
              ['accepted_as_is', profile.quality_counts.accepted_as_is],
              ['edit_detected', profile.quality_counts.edit_detected],
              ['heavy_edit', profile.quality_counts.heavy_edit],
              ['deleted_entire_output', profile.quality_counts.deleted_entire_output],
              ['re_recorded_quickly', profile.quality_counts.re_recorded_quickly],
              ['failures', profile.quality_counts.failures],
            ]}
          />
        </div>
        <div className="card p-4">
          <SectionLabel>Learning funnel</SectionLabel>
          <CountGrid
            items={[
              ['learning_candidate', profile.learning.learning_candidate],
              ['learning_modal_shown', profile.learning.learning_modal_shown],
              ['learning_confirmed', profile.learning.learning_confirmed],
              ['learning_dismissed', profile.learning.learning_dismissed],
              ['server_learning_saved', profile.learning.server_learning_saved],
              ['server_learning_blocked', profile.learning.server_learning_blocked],
            ]}
          />
        </div>
        <div className="card p-4">
          <SectionLabel>Latency percentiles (ms)</SectionLabel>
          <div className="text-[12px] space-y-1.5">
            <div className="flex justify-between">
              <span className="text-fg-4">transcribe</span>
              <span className="tabular-nums">
                {ms(lat.transcribe_p50)} / {ms(lat.transcribe_p95)}
              </span>
            </div>
            <div className="flex justify-between">
              <span className="text-fg-4">embed</span>
              <span className="tabular-nums">
                {ms(lat.embed_p50)} / {ms(lat.embed_p95)}
              </span>
            </div>
            <div className="flex justify-between">
              <span className="text-fg-4">polish</span>
              <span className="tabular-nums">
                {ms(lat.polish_p50)} / {ms(lat.polish_p95)}
              </span>
            </div>
            <div className="flex justify-between">
              <span className="text-fg-4">paste</span>
              <span className="tabular-nums">
                {ms(lat.paste_p50)} / {ms(lat.paste_p95)}
              </span>
            </div>
          </div>
        </div>
        <div className="card p-4">
          <SectionLabel>By mode · by app</SectionLabel>
          <div className="text-[12px] space-y-1.5 mb-3">
            {profile.by_mode.map(row => (
              <div key={row.mode} className="flex justify-between">
                <span className="font-mono text-fg-2">{row.mode}</span>
                <span className="tabular-nums">{row.count}</span>
              </div>
            ))}
            {!profile.by_mode.length && (
              <div className="text-fg-4">No runs in window.</div>
            )}
          </div>
          <div className="border-t border-border-light pt-3 text-[12px] space-y-1.5">
            {profile.by_target_app.map((row, i) => (
              <div key={i} className="flex justify-between">
                <span className="truncate max-w-[70%]">{row.target_app || 'Unknown'}</span>
                <span className="tabular-nums">{row.count}</span>
              </div>
            ))}
          </div>
        </div>
      </div>

      {profile.stt?.by_provider?.length ? (
        <div className="card p-4 mb-4">
          <SectionLabel>STT provider mix</SectionLabel>
          <div className="grid grid-cols-2 gap-4 text-[12px]">
            <div className="space-y-1.5">
              {profile.stt.by_provider.map(row => (
                <div key={row.stt_provider} className="flex justify-between">
                  <span className="font-mono text-fg-2">{row.stt_provider}</span>
                  <span className="tabular-nums">
                    {row.count} ({row.share}%)
                  </span>
                </div>
              ))}
            </div>
            <div className="space-y-1.5">
              {(profile.stt.latency_by_provider || []).map(row => (
                <div key={row.stt_provider} className="flex justify-between text-fg-3">
                  <span className="font-mono">{row.stt_provider} transcribe</span>
                  <span className="tabular-nums">
                    {ms(row.transcribe_p50)} / {ms(row.transcribe_p95)}
                  </span>
                </div>
              ))}
            </div>
          </div>
        </div>
      ) : null}

      {flagEntries.length > 0 && (
        <div className="card p-4 mb-4">
          <SectionLabel>Content flags (aggregate)</SectionLabel>
          <div className="flex flex-wrap gap-1.5">
            {flagEntries.map(([k, v]) => (
              <span
                key={k}
                className="text-[10px] px-2 py-0.5 rounded bg-info-bg text-info"
              >
                {k} · {v}
              </span>
            ))}
          </div>
        </div>
      )}

      <div className="card !p-0 overflow-hidden mb-4">
        <div className="px-5 py-4 border-b border-border">
          <SectionLabel>Daily rollups</SectionLabel>
          <p className="text-[11px] text-fg-4">runtime_telemetry_daily</p>
        </div>
        <table className="w-full">
          <thead>
            <tr>
              {[
                'Date',
                'Mode',
                'Runs',
                'Audio sec',
                'Accepted',
                'Edits',
                'Heavy',
                'Learning shown',
                'Confirmed',
                'Failures',
                'Fallbacks',
              ].map(h => (
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
            {profile.daily_rollups.map((row, i) => (
              <tr key={i}>
                <td className="text-[12px] px-5 py-2.5 border-b border-border-light">{row.event_date}</td>
                <td className="text-[11px] font-mono px-5 py-2.5 border-b border-border-light">{row.mode}</td>
                <td className="text-[12px] tabular-nums px-5 py-2.5 border-b border-border-light">{row.run_count}</td>
                <td className="text-[12px] tabular-nums px-5 py-2.5 border-b border-border-light">{Math.round(row.audio_seconds)}</td>
                <td className="text-[12px] tabular-nums px-5 py-2.5 border-b border-border-light">{row.accepted_count}</td>
                <td className="text-[12px] tabular-nums px-5 py-2.5 border-b border-border-light">{row.edit_count}</td>
                <td className="text-[12px] tabular-nums px-5 py-2.5 border-b border-border-light">{row.heavy_edit_count}</td>
                <td className="text-[12px] tabular-nums px-5 py-2.5 border-b border-border-light">{row.learning_modal_shown}</td>
                <td className="text-[12px] tabular-nums px-5 py-2.5 border-b border-border-light">{row.learning_confirmed}</td>
                <td className="text-[12px] tabular-nums px-5 py-2.5 border-b border-border-light">{row.failure_count}</td>
                <td className="text-[12px] tabular-nums px-5 py-2.5 border-b border-border-light">{row.fallback_count}</td>
              </tr>
            ))}
            {!profile.daily_rollups.length && (
              <tr>
                <td colSpan={11} className="text-[12px] text-fg-4 px-5 py-4">
                  No daily rollups yet.
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>

      <div className="card !p-0 overflow-hidden mb-4">
        <div className="px-5 py-4 border-b border-border">
          <SectionLabel>Upload batches</SectionLabel>
          <p className="text-[11px] text-fg-4">runtime_telemetry_uploads</p>
        </div>
        <table className="w-full">
          <thead>
            <tr>
              {['Received', 'device_id', 'client_version', 'runs', 'rollups', 'accepted', 'rejected'].map(
                h => (
                  <th
                    key={h}
                    className="text-[10px] font-medium text-fg-4 text-left px-5 py-3 border-b border-border uppercase tracking-wider"
                  >
                    {h}
                  </th>
                ),
              )}
            </tr>
          </thead>
          <tbody>
            {profile.uploads.map((u, i) => (
              <tr key={i}>
                <td className="text-[12px] px-5 py-2.5 border-b border-border-light">
                  {new Date(u.received_at).toLocaleString()}
                </td>
                <td className="text-[11px] font-mono px-5 py-2.5 border-b border-border-light">
                  {u.device_id || '—'}
                </td>
                <td className="text-[11px] font-mono px-5 py-2.5 border-b border-border-light">
                  {u.client_version || '—'}
                </td>
                <td className="text-[12px] tabular-nums px-5 py-2.5 border-b border-border-light">{u.run_count}</td>
                <td className="text-[12px] tabular-nums px-5 py-2.5 border-b border-border-light">{u.rollup_count}</td>
                <td className="text-[12px] tabular-nums px-5 py-2.5 border-b border-border-light">{u.accepted_count}</td>
                <td className="text-[12px] tabular-nums px-5 py-2.5 border-b border-border-light">{u.rejected_count}</td>
              </tr>
            ))}
            {!profile.uploads.length && (
              <tr>
                <td colSpan={7} className="text-[12px] text-fg-4 px-5 py-4">
                  No uploads recorded yet.
                </td>
              </tr>
            )}
          </tbody>
        </table>
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
        <table className="w-full">
          <thead>
            <tr>
              <th className="w-6" />
              {['event_at', 'run_id', 'mode', 'target_app', 'audio', 'words', 'total_ms', 'edit', 'ok', 'flags'].map(
                h => (
                  <th
                    key={h}
                    className="text-[10px] font-medium text-fg-4 text-left px-3 py-3 border-b border-border uppercase tracking-wider"
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
              const flagSummary = run.content_flags
                ? Object.entries(run.content_flags)
                    .filter(([, v]) => v)
                    .map(([k]) => k.replace('has_', ''))
                    .slice(0, 3)
                    .join(', ')
                : ''
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
                    <td className="text-[11px] font-mono px-3 py-2.5 border-b border-border-light">
                      {new Date(run.event_at).toLocaleString()}
                    </td>
                    <td className="text-[11px] font-mono px-3 py-2.5 border-b border-border-light">
                      {run.run_id}
                    </td>
                    <td className="text-[11px] font-mono px-3 py-2.5 border-b border-border-light">
                      {run.mode}
                    </td>
                    <td className="text-[12px] px-3 py-2.5 border-b border-border-light">
                      {run.target_app || '—'}
                    </td>
                    <td className="text-[12px] tabular-nums px-3 py-2.5 border-b border-border-light">
                      {run.audio_seconds != null ? `${run.audio_seconds}s` : '—'}
                    </td>
                    <td className="text-[12px] tabular-nums px-3 py-2.5 border-b border-border-light">
                      {run.word_count ?? '—'}
                    </td>
                    <td className="text-[12px] tabular-nums px-3 py-2.5 border-b border-border-light">
                      {run.total_ms ?? '—'}
                    </td>
                    <td className="px-3 py-2.5 border-b border-border-light">
                      <span className={`text-[10px] font-semibold px-2 py-0.5 rounded-full ${bucketClass}`}>
                        {run.edit_bucket}
                      </span>
                    </td>
                    <td className="px-3 py-2.5 border-b border-border-light">
                      <span
                        className={`text-[10px] font-semibold px-2 py-0.5 rounded-full ${
                          run.success ? 'bg-ok-bg text-ok' : 'bg-live-bg text-live'
                        }`}
                      >
                        {run.success ? 'ok' : 'fail'}
                      </span>
                    </td>
                    <td className="text-[10px] text-fg-4 px-3 py-2.5 border-b border-border-light">
                      {flagSummary || '—'}
                    </td>
                  </tr>
                  {open && (
                    <tr>
                      <td colSpan={11} className="p-0 border-b border-border-light">
                        <RunDetailPanel run={run} />
                      </td>
                    </tr>
                  )}
                </Fragment>
              )
            })}
            {!runs.length && !runsLoading && (
              <tr>
                <td colSpan={11} className="text-[12px] text-fg-4 px-5 py-4">
                  No runs in this window.
                </td>
              </tr>
            )}
          </tbody>
        </table>
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
