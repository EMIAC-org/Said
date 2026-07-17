import { useEffect, useState } from 'react'
import { apiJson } from '../api'
import { useAuth } from '../hooks/useAuth'
import { useWindowRange, winDays } from '../lib/window'
import { usd, timeAgo, firstName, personName, shortId } from '../lib/format'
import { Avatar, Loading, ErrorBox, Empty, TruncatedChip } from '../components/ui'
import { useDrawer } from '../components/Drawer'
import { RunDrawerHead, RunDrawerBody } from '../components/RunDrawer'
import type { OrgRun, OrgRunsResponse } from '../lib/adminTypes'

export function RunsPage() {
  const { org, platformAdmin, adminScopeOrgId } = useAuth()
  const { win } = useWindowRange()
  const drawer = useDrawer()
  const [data, setData] = useState<OrgRunsResponse | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')

  const orgId = org?.org?.id

  useEffect(() => {
    if (!orgId) { setLoading(false); return }
    setLoading(true)
    setError('')
    const path = platformAdmin
      ? `/v1/platform/runs?days=${winDays(win)}&limit=100${adminScopeOrgId ? `&org_id=${adminScopeOrgId}` : ''}`
      : `/v1/orgs/${orgId}/runs?days=${winDays(win)}&limit=100`
    apiJson<OrgRunsResponse>(path)
      .then(setData)
      .catch(e => setError(e.message))
      .finally(() => setLoading(false))
  }, [orgId, platformAdmin, adminScopeOrgId, win])

  function openRun(run: OrgRun) {
    const runOrgId = run.org_id ?? orgId
    if (!runOrgId) return
    drawer.open({
      head: <RunDrawerHead run={run} onClose={drawer.close} />,
      body: <RunDrawerBody run={run} orgId={runOrgId} />,
    })
  }

  return (
    <>
      <div className="page-head">
        <h1>Runs</h1>
        <p>Every dictation run — model, latency and cost. Click a run for its full trace.</p>
      </div>

      {loading ? <Loading /> : error ? <ErrorBox title="Failed to load runs" message={error} /> : (
        <div className="card">
          <div className="card-head">
            <div className="card-title">Dictation runs <span style={{ color: 'var(--muted)', fontWeight: 400 }}>· {data?.runs.length ?? 0} shown</span></div>
          </div>
          {!data || data.runs.length === 0 ? (
            <Empty title="No runs in this window" message="Try a wider time range." />
          ) : (
            <table className="table-fixed runs-table">
              <colgroup>
                <col className="col-run" />
                <col className="col-person" />
                {platformAdmin && <col className="col-workspace" />}
                <col className="col-app" />
                <col className="col-model" />
                <col className="col-model" />
                <col className="col-latency" />
                <col className="col-words" />
                <col className="col-cost" />
                <col className="col-arrow" />
              </colgroup>
              <thead>
                <tr>
                  <th>Run</th><th>Person</th>{platformAdmin && <th>Workspace</th>}<th>App</th><th>STT model</th><th>Polish model</th>
                  <th className="r">Latency</th><th className="r">Words</th><th className="r">Cost</th><th></th>
                </tr>
              </thead>
              <tbody>
                {data.runs.map(r => (
                  <tr key={`${r.org_id ?? orgId}:${r.run_id}`} className="clickable" onClick={() => openRun(r)}>
                    <td>
                      <div className="cell-strong mono truncate" style={{ fontSize: 11.5 }} title={r.run_id}>{shortId(r.run_id)}</div>
                      <div style={{ fontSize: 11, color: 'var(--muted)' }}>{timeAgo(r.event_at)}</div>
                    </td>
                    <td><div className="person-cell"><Avatar name={personName(r.lark_name, r.email)} size={24} /><span className="nm" style={{ fontSize: 12.5 }}>{firstName(r.name || personName(r.lark_name, r.email))}</span></div></td>
                    {platformAdmin && <td><TruncatedChip value={r.org_name || r.org_slug || 'Unknown'} /></td>}
                    <td><TruncatedChip value={r.target_app || 'Unknown'} /></td>
                    <td><TruncatedChip value={r.speech_model || 'local'} mono /></td>
                    <td><TruncatedChip value={r.polish_attempts?.[0]?.model || '—'} mono /></td>
                    <td className="r mono" style={{ fontSize: 11.5, color: 'var(--body)' }}>{r.total_ms ?? '—'}ms</td>
                    <td className="r tnum">{r.word_count ?? 0}</td>
                    <td className="r">
                      <span className="cost">{usd(r.total_cost_usd)}</span>
                      {r.cost_coverage !== 'complete' && <span className="tag neutral" style={{ marginLeft: 4 }}>est</span>}
                    </td>
                    <td className="r" style={{ color: 'var(--muted-soft)' }}>›</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
      )}
    </>
  )
}
