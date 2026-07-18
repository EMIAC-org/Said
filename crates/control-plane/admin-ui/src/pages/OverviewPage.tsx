import { useEffect, useState } from 'react'
import { useNavigate } from 'react-router'
import { apiJson } from '../api'
import { useAuth } from '../hooks/useAuth'
import { useWindowRange, WIN_LABEL, winDays } from '../lib/window'
import { usd, usd2, num, osLabel, osGlyph, firstName, shortId } from '../lib/format'
import { StatTile, Sparkline, SplitBar, Avatar, Loading, ErrorBox, TruncatedChip } from '../components/ui'
import type { AdminOverview } from '../lib/adminTypes'

export function OverviewPage() {
  const { org } = useAuth()
  const { win } = useWindowRange()
  const navigate = useNavigate()
  const [data, setData] = useState<AdminOverview | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')

  useEffect(() => {
    const orgId = org?.org?.id
    if (!orgId) { setLoading(false); return }
    setLoading(true)
    apiJson<AdminOverview>(`/v1/orgs/${orgId}/admin/overview?days=${winDays(win)}`)
      .then(setData)
      .catch(e => setError(e.message))
      .finally(() => setLoading(false))
  }, [org, win])

  if (loading) return <Loading />
  if (error) return <ErrorBox title="Failed to load overview" message={error} />
  if (!data) return null

  const { spend, totals } = data
  const tot = spend.total_usd || 1

  return (
    <>
      <div className="page-head">
        <h1>Overview</h1>
        <p>Fleet-wide dictation &amp; meeting activity · <b className="tnum">{WIN_LABEL[win]}</b></p>
      </div>

      <div className="grid g-4">
        <StatTile label="Total spend" value={usd2(spend.total_usd)} sub={`${usd2(spend.stt_usd + spend.polish_usd)} dictation · ${usd2(spend.meeting_usd)} meetings`} />
        <StatTile label="Dictation runs" value={num(totals.runs)} sub={`${num(totals.words)} words polished`} />
        <StatTile label="Words dictated" value={num(totals.words)} sub={`across ${totals.total_people} people`} />
        <StatTile label="Active now" value={`${totals.active_people} / ${totals.total_people}`} sub="seen in last 15 min" />
      </div>

      <div className="grid g-main mt">
        <div className="card">
          <div className="card-head">
            <div className="card-title">Dictation volume</div>
            <div className="chip mono">{WIN_LABEL[win]}</div>
          </div>
          <div className="card-pad">
            <Sparkline values={data.volume.length ? data.volume.map(v => v.runs) : [0, 0]} />
          </div>
        </div>

        <div className="card">
          <div className="card-head"><div className="card-title">Cost breakdown</div></div>
          <div className="card-pad">
            <div style={{ fontSize: 28, letterSpacing: '-.03em', marginBottom: 4 }} className="tnum">{usd2(spend.total_usd)}</div>
            <div style={{ fontSize: 12, color: 'var(--muted)', marginBottom: 16 }}>total spend · {WIN_LABEL[win]}</div>
            <SplitBar segments={[
              { pct: (spend.stt_usd / tot) * 100, color: 'var(--tl-thinking)' },
              { pct: (spend.polish_usd / tot) * 100, color: 'var(--tl-read)' },
              { pct: (spend.meeting_usd / tot) * 100, color: 'var(--tl-done)' },
            ]} />
            <div className="legend">
              <div className="li"><span className="sw" style={{ background: 'var(--tl-thinking)' }} /> STT {usd2(spend.stt_usd)}</div>
              <div className="li"><span className="sw" style={{ background: 'var(--tl-read)' }} /> Polish {usd2(spend.polish_usd)}</div>
              <div className="li"><span className="sw" style={{ background: 'var(--tl-done)' }} /> Meetings {usd2(spend.meeting_usd)}</div>
            </div>
            <div className="hint" style={{ marginTop: 16 }}>
              Meeting spend comes only from recorded provider usage and its stored historical rate snapshot. Meetings without provider usage add zero spend.
            </div>
          </div>
        </div>
      </div>

      <div className="grid g-main mt">
        <div className="card">
          <div className="card-head">
            <div className="card-title">Top people by spend</div>
            <div className="chip click" onClick={() => navigate('/people')}>View all →</div>
          </div>
          <table>
            <thead><tr><th>Person</th><th>Device</th><th className="r">Runs</th><th className="r">Dictation</th><th className="r">Meetings</th><th className="r">Total</th></tr></thead>
            <tbody>
              {data.top_people.map(p => (
                <tr key={p.account_id} className="clickable" onClick={() => navigate(`/people/${p.account_id}`)}>
                  <td><div className="person-cell"><Avatar name={p.name} size={26} /><div className="nm">{p.name}</div></div></td>
                  <td><span className="os"><span className="glyph">{osGlyph(p.platform)}</span>{osLabel(p.platform)}{p.app_version ? <> · <span className="mono" style={{ fontSize: 11 }}>v{p.app_version}</span></> : null}</span></td>
                  <td className="r tnum">{num(p.runs)}</td>
                  <td className="r"><span className="cost">{usd2(p.dictation_usd)}</span></td>
                  <td className="r"><span className="cost">{usd2(p.meeting_usd)}</span></td>
                  <td className="r"><span className="cost">{usd2(p.total_usd)}</span></td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>

        <div className="card">
          <div className="card-head">
            <div className="card-title">Recent runs</div>
            <div className="chip click" onClick={() => navigate('/runs')}>View all →</div>
          </div>
          <table className="table-fixed overview-runs-table">
            <colgroup><col className="col-run" /><col /><col className="col-cost" /></colgroup>
            <thead><tr><th>Run</th><th>App</th><th className="r">Cost</th></tr></thead>
            <tbody>
              {data.recent_runs.map(r => (
                <tr key={r.run_id} className="clickable" onClick={() => navigate('/runs')}>
                  <td>
                    <div className="cell-strong mono truncate" style={{ fontSize: 11.5 }} title={r.run_id}>{shortId(r.run_id)}</div>
                    <div style={{ fontSize: 11, color: 'var(--muted)' }}>{firstName(r.name)} · {r.word_count ?? 0}w</div>
                  </td>
                  <td><TruncatedChip value={r.target_app || 'Unknown'} /></td>
                  <td className="r"><span className="cost">{usd(r.total_cost_usd)}</span></td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>
    </>
  )
}
