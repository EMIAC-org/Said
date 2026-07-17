import { useEffect, useState } from 'react'
import { useNavigate, useParams, useSearchParams } from 'react-router'
import { apiJson } from '../api'
import { useAuth } from '../hooks/useAuth'
import { useWindowRange, WIN_LABEL, winDays } from '../lib/window'
import { usd, usd2, num } from '../lib/format'
import { osLabel, osGlyph } from '../lib/format'
import { StatTile, Sparkline, SplitBar, Avatar, Loading, ErrorBox } from '../components/ui'
import { useDrawer } from '../components/Drawer'
import { RunDrawerHead, RunDrawerBody } from '../components/RunDrawer'
import { MeetingDrawerHead, MeetingDrawerBody } from '../components/MeetingDrawer'
import type { PersonDetail, OrgRun, PersonMeetingRow, MeetingCostRow } from '../lib/adminTypes'
import type { TelemetryRun } from '../types'

export function PersonDetailPage() {
  const { id } = useParams<{ id: string }>()
  const [searchParams] = useSearchParams()
  const { org } = useAuth()
  const { win } = useWindowRange()
  const navigate = useNavigate()
  const drawer = useDrawer()
  const [data, setData] = useState<PersonDetail | null>(null)
  const [runs, setRuns] = useState<TelemetryRun[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')

  const orgId = searchParams.get('org') || org?.org?.id

  useEffect(() => {
    if (!orgId || !id) {
      setData(null)
      setRuns([])
      setError('')
      setLoading(false)
      return
    }
    let active = true
    setLoading(true)
    setError('')
    const days = winDays(win)
    Promise.all([
      apiJson<PersonDetail>(`/v1/orgs/${orgId}/telemetry/users/${id}?days=${days}`),
      apiJson<{ runs: TelemetryRun[] }>(`/v1/orgs/${orgId}/telemetry/users/${id}/runs?days=${days}&limit=6`).catch(() => ({ runs: [] })),
    ])
      .then(([detail, runsRes]) => {
        if (active) {
          setData(detail)
          setRuns(runsRes.runs || [])
        }
      })
      .catch(error => {
        if (active) setError(error instanceof Error ? error.message : 'Unable to load person.')
      })
      .finally(() => { if (active) setLoading(false) })
    return () => { active = false }
  }, [orgId, id, win])

  if (loading) return <Loading />
  if (error) return <ErrorBox title="Failed to load person" message={error} />
  if (!data) return null

  const m = data.member
  const name = m.lark_name || m.email?.split('@')[0] || 'Unknown'
  const stt = data.costs.summary.stt_usd
  const pol = data.costs.summary.polish_usd
  const meet = data.meeting_cost_usd ?? 0
  const dict = stt + pol
  const total = dict + meet || 1
  const primarySpeech = data.speech?.by_model?.[0]?.speech_model || m.auth_source

  function openRun(r: TelemetryRun) {
    if (!orgId) return
    const orgRun: OrgRun = { ...r, account_id: id!, name, email: m.email }
    drawer.open({
      head: <RunDrawerHead run={orgRun} onClose={drawer.close} />,
      body: <RunDrawerBody run={orgRun} orgId={orgId} />,
    })
  }

  function openMeeting(meeting: PersonMeetingRow) {
    if (!orgId || !id) return
    const row: MeetingCostRow = {
      ...meeting,
      created_at: meeting.started_at,
      ended_at: null,
      host_account_id: id,
      host_name: name,
      host_email: m.email,
      participant_count: 1,
      provider: meeting.provider,
      usage_count: meeting.usage_count,
      cached_input_tokens: 0,
      cache_miss_tokens: meeting.input_tokens,
      reasoning_tokens: 0,
    }
    drawer.open({
      head: <MeetingDrawerHead row={row} onClose={drawer.close} />,
      body: <MeetingDrawerBody row={row} orgId={orgId} onOpenPerson={() => drawer.close()} />,
    })
  }

  return (
    <>
      <button className="back" onClick={() => navigate('/people')}>‹ Back to people</button>

      <div className="pd-head">
        <Avatar name={name} size={52} />
        <div style={{ flex: 1 }}>
          <h1>{name}</h1>
          <div className="em">{m.email} · {m.desktop_active ? <span style={{ color: 'var(--success)' }}>● active</span> : 'idle'}</div>
        </div>
        <div style={{ textAlign: 'right' }}>
          <div className="os" style={{ justifyContent: 'flex-end' }}>
            <span className="glyph">{osGlyph(m.platform)}</span> {osLabel(m.platform)}
            {m.app_version ? <> · <span className="mono" style={{ fontSize: 12 }}>v{m.app_version}</span></> : null}
          </div>
          <div style={{ fontSize: 11.5, color: 'var(--muted)', marginTop: 3 }}>primary STT: <span className="mono">{primarySpeech}</span></div>
        </div>
      </div>

      <div className="kv k4">
        <div className="cell"><div className="k">Runs</div><div className="v tnum">{num(data.summary.runs)}</div></div>
        <div className="cell"><div className="k">Words dictated</div><div className="v tnum">{num(data.summary.word_count)}</div></div>
        <div className="cell"><div className="k">Audio dictated</div><div className="v tnum">{Math.round(data.summary.audio_minutes)} <small>min</small></div></div>
        <div className="cell"><div className="k">Acceptance</div><div className="v tnum">{Math.round(data.quality.acceptance_rate)}<small>%</small></div></div>
      </div>

      <div className="section-label">Meetings · {WIN_LABEL[win]}</div>
      <div className="kv k4">
        <div className="cell"><div className="k">Meetings</div><div className="v tnum">{num(data.meeting_count ?? data.meetings_hosted ?? 0)}</div></div>
        <div className="cell"><div className="k">Recording</div><div className="v tnum">{Math.round((data.meeting_duration_seconds ?? 0) / 60)} <small>min</small></div></div>
        <div className="cell"><div className="k">Transcript</div><div className="v tnum">{num(data.meeting_transcript_words ?? 0)} <small>words</small></div></div>
        <div className="cell"><div className="k">AI spend</div><div className="v tnum">{usd2(meet)}</div></div>
      </div>

      <div className="section-label">Cost · {WIN_LABEL[win]}</div>
      <div className="grid g-3">
        <StatTile label="Dictation" value={usd2(dict)} sub={`${usd(stt)} STT · ${usd(pol)} polish`} />
        <StatTile label="Meetings" value={usd2(meet)} sub={`${data.meetings_hosted ?? 0} meetings hosted`} />
        <StatTile label="Total" value={usd2(dict + meet)} sub="all AI spend" />
      </div>

      <div className="grid g-main mt">
        <div className="card">
          <div className="card-head"><div className="card-title">Daily spend</div><div className="chip mono">{WIN_LABEL[win]}</div></div>
          <div className="card-pad">
            <Sparkline height={110} values={data.costs.daily.length ? data.costs.daily.map(d => d.total_usd) : [0, 0]} />
          </div>
        </div>
        <div className="card">
          <div className="card-head"><div className="card-title">Cost split</div></div>
          <div className="card-pad">
            <div style={{ marginBottom: 14 }}>
              <SplitBar segments={[
                { pct: (stt / total) * 100, color: 'var(--tl-thinking)' },
                { pct: (pol / total) * 100, color: 'var(--tl-read)' },
                { pct: (meet / total) * 100, color: 'var(--tl-done)' },
              ]} />
            </div>
            <div className="legend" style={{ flexDirection: 'column', gap: 9 }}>
              <div className="li"><span className="sw" style={{ background: 'var(--tl-thinking)' }} /> STT — <span className="cost cost-mut">{usd(stt)}</span></div>
              <div className="li"><span className="sw" style={{ background: 'var(--tl-read)' }} /> Polish — <span className="cost cost-mut">{usd(pol)}</span></div>
              <div className="li"><span className="sw" style={{ background: 'var(--tl-done)' }} /> Meetings — <span className="cost cost-mut">{usd2(meet)}</span></div>
            </div>
          </div>
        </div>
      </div>

      <div className="section-label">Recent meetings</div>
      <div className="card">
        {(data.recent_meetings ?? []).length === 0 ? (
          <div className="card-pad" style={{ color: 'var(--muted)', fontSize: 13 }}>No meetings in this window.</div>
        ) : (
          <table>
            <thead><tr><th>Date</th><th>Meeting</th><th className="r">Duration</th><th className="r">Words</th><th>Model</th><th className="r">Cost</th></tr></thead>
            <tbody>
              {(data.recent_meetings ?? []).map(meeting => (
                <tr key={`${meeting.source}-${meeting.id}`} className="clickable" onClick={() => openMeeting(meeting)}>
                  <td>{new Date(meeting.started_at).toLocaleDateString('en-US', { month: 'short', day: 'numeric' })}</td>
                  <td className="cell-strong">{meeting.title}<div className="em">{meeting.source === 'local' ? 'Local desktop' : 'Historical cloud'}</div></td>
                  <td className="r tnum">{Math.round(meeting.duration_seconds / 60)}m</td>
                  <td className="r tnum">{num(meeting.transcript_word_count)}</td>
                  <td><span className="chip mono">{meeting.usage_count === 0 ? 'No AI usage' : meeting.model || 'Unknown model'}</span></td>
                  <td className="r"><span className="cost">{usd2(meeting.cost_usd)}</span></td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>

      <div className="section-label">Recent runs</div>
      <div className="card">
        {runs.length === 0 ? (
          <div className="card-pad" style={{ color: 'var(--muted)', fontSize: 13 }}>No runs in this window.</div>
        ) : (
          <table>
            <thead><tr><th>Run</th><th>App</th><th>Model</th><th className="r">Words</th><th className="r">Cost</th></tr></thead>
            <tbody>
              {runs.map(r => (
                <tr key={r.run_id} className="clickable" onClick={() => openRun(r)}>
                  <td className="mono cell-strong" style={{ fontSize: 11.5 }}>{r.run_id}</td>
                  <td><span className="chip">{r.target_app || 'Unknown'}</span></td>
                  <td><span className="chip mono">{r.speech_model || 'local'}</span></td>
                  <td className="r tnum">{r.word_count ?? 0}</td>
                  <td className="r"><span className="cost">{usd(r.total_cost_usd)}</span></td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </>
  )
}
