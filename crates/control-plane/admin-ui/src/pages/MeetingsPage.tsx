import { useEffect, useState } from 'react'
import { useNavigate } from 'react-router'
import { apiJson } from '../api'
import { useAuth } from '../hooks/useAuth'
import { useWindowRange, winDays } from '../lib/window'
import { usd2, num, firstName } from '../lib/format'
import { StatTile, Avatar, Loading, ErrorBox, Empty } from '../components/ui'
import { useDrawer } from '../components/Drawer'
import { MeetingDrawerHead, MeetingDrawerBody } from '../components/MeetingDrawer'
import type { OrgMeetingCosts, MeetingCostRow } from '../lib/adminTypes'

function duration(seconds: number): string {
  if (!seconds) return '0m'
  const minutes = Math.round(seconds / 60)
  if (minutes < 60) return `${minutes}m`
  return `${Math.floor(minutes / 60)}h ${minutes % 60}m`
}

export function MeetingsPage() {
  const { org } = useAuth()
  const { win } = useWindowRange()
  const navigate = useNavigate()
  const drawer = useDrawer()
  const [data, setData] = useState<OrgMeetingCosts | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')

  const orgId = org?.org?.id

  useEffect(() => {
    if (!orgId) {
      setData(null)
      setError('')
      setLoading(false)
      return
    }
    let active = true
    setLoading(true)
    setError('')
    apiJson<OrgMeetingCosts>(`/v1/orgs/${orgId}/meetings/costs?days=${winDays(win)}`)
      .then(result => { if (active) setData(result) })
      .catch(error => {
        if (active) setError(error instanceof Error ? error.message : 'Unable to load meetings.')
      })
      .finally(() => { if (active) setLoading(false) })
    return () => { active = false }
  }, [orgId, win])

  function openMeeting(row: MeetingCostRow) {
    if (!orgId) return
    drawer.open({
      head: <MeetingDrawerHead row={row} onClose={drawer.close} />,
      body: <MeetingDrawerBody row={row} orgId={orgId} onOpenPerson={(a) => { drawer.close(); navigate(`/people/${a}`) }} />,
    })
  }

  const meetings = data?.meetings ?? []
  const usageCount = meetings.reduce((sum, meeting) => sum + meeting.usage_count, 0)

  return (
    <>
      <div className="page-head">
        <h1>Meetings</h1>
        <p>Unified historical cloud meetings and local desktop sessions. Provider and model labels come from recorded AI usage.</p>
      </div>

      {loading ? <Loading /> : error ? <ErrorBox title="Failed to load meetings" message={error} /> : (
        <>
          <div className="grid g-5">
            <StatTile label="Meetings" value={num(data?.meeting_count ?? meetings.length)} sub="in view" />
            <StatTile label="Recording" value={duration(data?.total_recording_seconds ?? 0)} sub="meeting audio" />
            <StatTile label="Transcript" value={num(data?.total_transcript_words ?? 0)} sub="words" />
            <StatTile label="Tokens" value={num(data?.total_tokens ?? 0)} sub={usageCount === 0 ? 'no AI calls recorded' : 'prompt + completion'} />
            <StatTile label="Recorded AI spend" value={usd2(data?.total_cost_usd ?? 0)} sub={usageCount === 0 ? 'no billed AI calls in view' : `${num(usageCount)} DeepSeek call${usageCount === 1 ? '' : 's'}`} />
          </div>

          <div className="card mt">
            <div className="card-head"><div className="card-title">Meetings</div></div>
            {meetings.length === 0 ? (
              <Empty title="No meetings in this window" message="Try a wider time range." />
            ) : (
              <table>
                <thead>
                  <tr>
                    <th>Date</th><th>Meeting</th><th>Owner</th><th className="r">Duration</th>
                    <th className="r">Words</th><th>Model</th><th className="r">Tokens</th><th className="r">Cost</th><th>Status</th>
                  </tr>
                </thead>
                <tbody>
                  {meetings.map(mtg => (
                    <tr key={mtg.id} className="clickable" onClick={() => openMeeting(mtg)}>
                      <td style={{ whiteSpace: 'nowrap' }}>{new Date(mtg.started_at).toLocaleDateString('en-US', { month: 'short', day: 'numeric' })}</td>
                      <td className="cell-strong">{mtg.title}<div style={{ fontSize: 10.5, color: 'var(--muted)', fontWeight: 400 }}>{mtg.source === 'local' ? 'Local desktop' : 'Historical cloud'}</div></td>
                      <td><div className="person-cell"><Avatar name={mtg.host_name} size={24} /><span className="nm" style={{ fontSize: 12.5 }}>{firstName(mtg.host_name)}</span></div></td>
                      <td className="r tnum">{duration(mtg.duration_seconds)}</td>
                      <td className="r tnum">{num(mtg.transcript_word_count)}</td>
                      <td><span className="chip mono">{mtg.usage_count === 0 ? 'AI not run' : mtg.model || 'Unknown model'}</span></td>
                      <td className="r tnum mono" style={{ fontSize: 11.5 }}>{num(mtg.input_tokens + mtg.output_tokens)}</td>
                      <td className="r"><span className="cost">{mtg.usage_count === 0 ? '—' : usd2(mtg.cost_usd)}</span></td>
                      <td>{mtg.status === 'live' ? <span className="tag warn">● Live</span> : <span className="tag neutral">{mtg.status}</span>}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </div>

          {meetings.length > 0 && (data?.total_tokens ?? 0) === 0 && (
            <div className="hint" style={{ marginTop: 14 }}>Meetings exist in this window, but DeepSeek was not run or its token usage was not recorded. No cost is inferred without a provider usage event.</div>
          )}
          <div className="hint" style={{ marginTop: 14 }}>
            Meeting costs are server-priced from the rate-card snapshot stored with each usage event.
            {(data?.rate_cards ?? []).map(rate => (
              <span key={rate.model}> <b>{rate.model}</b>: ${rate.cache_hit_usd_per_million}/M cached input · ${rate.cache_miss_usd_per_million}/M uncached input · ${rate.output_usd_per_million}/M output.</span>
            ))}
          </div>
        </>
      )}
    </>
  )
}
