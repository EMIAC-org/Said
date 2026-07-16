import { useEffect, useState } from 'react'
import { useNavigate } from 'react-router'
import { apiJson } from '../api'
import { useAuth } from '../hooks/useAuth'
import { useWindowRange, winDays } from '../lib/window'
import { usd2, num, firstName } from '../lib/format'
import { MEET_MODEL, MEET_PROVIDER, MEET_IN_PER_M, MEET_OUT_PER_M } from '../lib/rates'
import { StatTile, Avatar, Loading, ErrorBox, Empty } from '../components/ui'
import { useDrawer } from '../components/Drawer'
import { MeetingDrawerHead, MeetingDrawerBody } from '../components/MeetingDrawer'
import type { OrgMeetingCosts, MeetingCostRow } from '../lib/adminTypes'

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
    if (!orgId) { setLoading(false); return }
    setLoading(true)
    apiJson<OrgMeetingCosts>(`/v1/orgs/${orgId}/meetings/costs?days=${winDays(win)}`)
      .then(setData)
      .catch(e => setError(e.message))
      .finally(() => setLoading(false))
  }, [orgId, win])

  function openMeeting(row: MeetingCostRow) {
    if (!orgId) return
    drawer.open({
      head: <MeetingDrawerHead row={row} onClose={drawer.close} />,
      body: <MeetingDrawerBody row={row} orgId={orgId} onOpenPerson={(a) => { drawer.close(); navigate(`/people/${a}`) }} />,
    })
  }

  const meetings = data?.meetings ?? []

  return (
    <>
      <div className="page-head">
        <h1>Meetings</h1>
        <p>Meeting summaries run on paid <b>{MEET_PROVIDER} {MEET_MODEL}</b>. Track who meets and what it costs.</p>
      </div>

      {loading ? <Loading /> : error ? <ErrorBox title="Failed to load meetings" message={error} /> : (
        <>
          <div className="grid g-3">
            <StatTile label="Meetings" value={num(meetings.length)} sub="in view" />
            <StatTile label="AI spend" value={usd2(data?.total_cost_usd ?? 0)} sub={MEET_MODEL} />
            <StatTile label="Tokens" value={num(data?.total_tokens ?? 0)} sub={`in + out to ${MEET_PROVIDER}`} />
          </div>

          <div className="card mt">
            <div className="card-head"><div className="card-title">Meetings</div></div>
            {meetings.length === 0 ? (
              <Empty title="No meetings in this window" message="Try a wider time range." />
            ) : (
              <table>
                <thead>
                  <tr>
                    <th>Meeting</th><th>Host</th><th className="r">People</th>
                    <th>Model</th><th className="r">Tokens</th><th className="r">Cost</th><th>Status</th>
                  </tr>
                </thead>
                <tbody>
                  {meetings.map(mtg => (
                    <tr key={mtg.id} className="clickable" onClick={() => openMeeting(mtg)}>
                      <td className="cell-strong">
                        {mtg.title}
                        <div style={{ fontSize: 11, color: 'var(--muted)', fontWeight: 400 }}>{new Date(mtg.created_at).toLocaleDateString('en-US', { month: 'short', day: 'numeric' })}</div>
                      </td>
                      <td><div className="person-cell"><Avatar name={mtg.host_name} size={24} /><span className="nm" style={{ fontSize: 12.5 }}>{firstName(mtg.host_name)}</span></div></td>
                      <td className="r tnum">{mtg.participant_count}</td>
                      <td><span className="chip mono">{MEET_MODEL}</span></td>
                      <td className="r tnum mono" style={{ fontSize: 11.5 }}>{num(mtg.input_tokens + mtg.output_tokens)}</td>
                      <td className="r"><span className="cost">{usd2(mtg.cost_usd)}</span></td>
                      <td>{mtg.status === 'live' ? <span className="tag warn">● Live</span> : <span className="tag neutral">Ended</span>}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </div>

          <div className="hint" style={{ marginTop: 14 }}>
            Meeting AI cost is <b>estimated</b> on the {MEET_PROVIDER} {MEET_MODEL} rate card (${MEET_IN_PER_M}/M in · ${MEET_OUT_PER_M}/M out). Token capture was newly wired into the summarizer — historical meetings before that show zero.
          </div>
        </>
      )}
    </>
  )
}
