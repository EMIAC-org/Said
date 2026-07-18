import { useEffect, useState } from 'react'
import { apiJson } from '../api'
import { usd2, num } from '../lib/format'
import { DrawerClose } from './Drawer'
import { Avatar, Loading } from './ui'
import type { MeetingCostDetail, MeetingCostRow } from '../lib/adminTypes'

function duration(seconds: number): string {
  const minutes = Math.round((seconds || 0) / 60)
  return minutes < 60 ? `${minutes}m` : `${Math.floor(minutes / 60)}h ${minutes % 60}m`
}

export function MeetingDrawerHead({ row, onClose }: { row: MeetingCostRow; onClose: () => void }) {
  return (
    <>
      <div>
        <div className="drawer-title">{row.title}</div>
        <div className="drawer-meta">{row.source === 'local' ? 'Local desktop' : 'Historical cloud'} · {new Date(row.started_at).toLocaleDateString()}</div>
      </div>
      <DrawerClose onClick={onClose} />
    </>
  )
}

export function MeetingDrawerBody({
  row,
  orgId,
  onOpenPerson,
}: {
  row: MeetingCostRow
  orgId: string
  onOpenPerson: (accountId: string) => void
}) {
  const [detail, setDetail] = useState<MeetingCostDetail | null>(null)
  const [loading, setLoading] = useState(true)
  const [fetchError, setFetchError] = useState('')
  const [requestVersion, setRequestVersion] = useState(0)

  useEffect(() => {
    let active = true
    setLoading(true)
    setDetail(null)
    setFetchError('')
    apiJson<MeetingCostDetail>(`/v1/orgs/${orgId}/meetings/${row.id}/cost`)
      .then(result => { if (active) setDetail(result) })
      .catch(error => {
        if (active) setFetchError(error instanceof Error ? error.message : 'Unable to load meeting detail.')
      })
      .finally(() => { if (active) setLoading(false) })
    return () => { active = false }
  }, [row.id, orgId, requestVersion])

  if (loading) return <Loading />
  if (fetchError || !detail) {
    return (
      <div className="errbox">
        <h3>Meeting detail unavailable</h3>
        <p>{fetchError || 'The meeting detail response was empty.'}</p>
        <button className="btn" type="button" onClick={() => setRequestVersion(version => version + 1)}>Retry</button>
      </div>
    )
  }

  const stages = detail.by_stage ?? []

  return (
    <>
      <div className="section-label first">Meeting metadata</div>
      <div className="kv">
        <div className="cell"><div className="k">Duration</div><div className="v tnum">{duration(row.duration_seconds)}</div></div>
        <div className="cell"><div className="k">Transcript</div><div className="v tnum">{num(row.transcript_word_count)} words</div></div>
        <div className="cell"><div className="k">Status</div><div className="v">{row.status}</div></div>
        <div className="cell"><div className="k">AI spend</div><div className="v tnum">{row.usage_count === 0 ? '—' : usd2(row.cost_usd)}</div></div>
      </div>

      <div className="section-label">Owner</div>
      <div className="card">
        <table>
          <tbody>
            <tr className="clickable" onClick={() => onOpenPerson(row.host_account_id)}>
              <td>
                <div className="person-cell">
                  <Avatar name={row.host_name} size={28} />
                  <div><div className="nm">{row.host_name}</div><div className="em">{row.host_email}</div></div>
                </div>
              </td>
              <td className="r" style={{ color: 'var(--muted-soft)' }}>›</td>
            </tr>
          </tbody>
        </table>
      </div>

      <div className="section-label">AI usage by stage</div>
      {stages.length === 0 ? (
        <div className="card card-pad" style={{ fontSize: 13 }}>
          <div className="cell-strong">AI not run</div>
          <div style={{ color: 'var(--muted)', marginTop: 4 }}>This meeting has no recorded provider call, so there are no tokens or costs to attribute.</div>
        </div>
      ) : (
        <div className="card">
          <table>
            <thead><tr><th>Stage</th><th>Model</th><th>Status</th><th className="r">Tokens</th><th className="r">Latency</th><th className="r">Cost</th></tr></thead>
            <tbody>
              {stages.map((stage, index) => (
                <tr key={`${stage.stage}-${stage.model}-${stage.result_status}-${index}`}>
                  <td className="cell-strong">{stage.stage}<div className="em">{stage.call_count} call{stage.call_count === 1 ? '' : 's'}</div></td>
                  <td><span className="chip mono">{stage.model}</span></td>
                  <td><span className={stage.result_status === 'success' ? 'tag neutral' : 'tag warn'}>{stage.result_status}</span></td>
                  <td className="r tnum">{num(stage.input_tokens + stage.output_tokens)}</td>
                  <td className="r tnum">{stage.average_latency_ms == null ? '—' : `${Math.round(stage.average_latency_ms)}ms`}</td>
                  <td className="r"><span className="cost">{usd2(stage.cost_usd)}</span></td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {stages.length > 0 && (
        <div className="hint" style={{ marginTop: 14 }}>
          Prompt tokens include {num(stages.reduce((sum, stage) => sum + stage.cached_input_tokens, 0))} cache hits and {num(stages.reduce((sum, stage) => sum + stage.cache_miss_tokens, 0))} cache misses. Completion totals already include reasoning tokens.
        </div>
      )}
    </>
  )
}
