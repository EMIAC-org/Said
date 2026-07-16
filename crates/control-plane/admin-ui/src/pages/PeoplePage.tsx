import { useEffect, useState } from 'react'
import { useNavigate } from 'react-router'
import { apiJson } from '../api'
import { useAuth } from '../hooks/useAuth'
import { useWindowRange, WIN_LABEL, winDays } from '../lib/window'
import { usd2, num, osLabel, osGlyph } from '../lib/format'
import { Avatar, Loading, ErrorBox, Empty } from '../components/ui'
import type { PersonRow, PeopleResponse } from '../lib/adminTypes'

function displayName(p: PersonRow): string {
  return p.lark_name || p.email?.split('@')[0] || 'Unknown'
}

export function PeoplePage() {
  const { org } = useAuth()
  const { win } = useWindowRange()
  const navigate = useNavigate()
  const [data, setData] = useState<PeopleResponse | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')

  const orgId = org?.org?.id

  useEffect(() => {
    if (!orgId) { setLoading(false); return }
    setLoading(true)
    apiJson<PeopleResponse>(`/v1/orgs/${orgId}/telemetry/users?days=${winDays(win)}&limit=200`)
      .then(setData)
      .catch(e => setError(e.message))
      .finally(() => setLoading(false))
  }, [orgId, win])

  const rows = data?.users ?? []

  return (
    <>
      <div className="page-head">
        <h1>People</h1>
        <p>Who’s using AirNote, on what device, and how much they cost · <b>{WIN_LABEL[win]}</b></p>
      </div>

      {loading ? <Loading /> : error ? <ErrorBox title="Failed to load people" message={error} /> : (
        <>
          <div className="card">
            <div className="card-head">
              <div className="card-title">{rows.length} people</div>
              <div className="search mono" style={{ width: 220 }}>⌘K Search people…</div>
            </div>
            {rows.length === 0 ? (
              <Empty title="No active people in this window" message="Try a wider time range." />
            ) : (
              <table>
                <thead>
                  <tr>
                    <th>Person</th><th>Device</th><th>Status</th>
                    <th className="r">Runs</th><th className="r">Words</th><th className="r">Audio</th>
                    <th className="r">Dictation</th><th className="r">Meetings</th><th className="r">Total cost</th>
                  </tr>
                </thead>
                <tbody>
                  {rows.map(p => {
                    const d = p.costs?.total_usd ?? 0
                    const m = p.meeting_cost_usd ?? 0
                    return (
                      <tr key={p.account_id} className="clickable" onClick={() => navigate(`/people/${p.account_id}`)}>
                        <td>
                          <div className="person-cell">
                            <Avatar name={displayName(p)} size={30} />
                            <div><div className="nm">{displayName(p)}</div><div className="em">{p.email}</div></div>
                          </div>
                        </td>
                        <td>
                          <span className="os">
                            <span className="glyph">{osGlyph(p.platform)}</span>{osLabel(p.platform)}
                            {p.app_version ? <> · <span className="mono" style={{ fontSize: 11 }}>v{p.app_version}</span></> : null}
                          </span>
                        </td>
                        <td>{p.desktop_active ? <span className="os"><span className="dot live" /> Active</span> : <span className="os"><span className="dot idle" /> Idle</span>}</td>
                        <td className="r tnum">{num(p.runs)}</td>
                        <td className="r tnum">{num(p.word_count ?? 0)}</td>
                        <td className="r tnum">{Math.round(p.audio_minutes)}m</td>
                        <td className="r"><span className="cost">{usd2(d)}</span></td>
                        <td className="r"><span className="cost">{usd2(m)}</span></td>
                        <td className="r"><span className="cost accent">{usd2(d + m)}</span></td>
                      </tr>
                    )
                  })}
                </tbody>
              </table>
            )}
          </div>
          <div className="hint" style={{ marginTop: 14 }}>
            Device shows the desktop platform &amp; app version (real fields: <span className="mono">platform</span>, <span className="mono">app_version</span>). OS version isn’t stored today — “macOS/Windows” is derived from platform.
          </div>
        </>
      )}
    </>
  )
}
