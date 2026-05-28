import { useEffect, useMemo, useState } from 'react'
import { AlertTriangle, Camera, CheckCircle2, ExternalLink, MessageSquareText } from 'lucide-react'
import { apiJson } from '../api'
import { Empty, ErrorBox, Loading } from '../components/States'
import { formatDate, timeAgo } from '../utils'
import type { BugReport } from '../types'

const statuses = ['open', 'triaged', 'fixed', 'closed'] as const

const statusClasses: Record<string, string> = {
  open: 'text-live bg-live-bg',
  triaged: 'text-warn bg-warn-bg',
  fixed: 'text-ok bg-ok-bg',
  closed: 'text-fg-3 bg-surface-4',
}

const severityClasses: Record<string, string> = {
  low: 'text-fg-3 bg-surface-4',
  normal: 'text-info bg-info-bg',
  high: 'text-warn bg-warn-bg',
  blocking: 'text-live bg-live-bg',
}

function Pill({ value, kind }: { value: string; kind: 'status' | 'severity' }) {
  const styles = kind === 'status' ? statusClasses : severityClasses
  const cls = styles[value] || 'text-fg-3 bg-surface-4'
  return (
    <span className={`inline-flex items-center text-[10px] font-semibold px-2.5 py-1 rounded-full uppercase tracking-wide ${cls}`}>
      {value}
    </span>
  )
}

function reportImage(report: BugReport): string | null {
  return report.screenshot_url || report.screenshot_data_url || null
}

function reporter(report: BugReport): string {
  return report.reporter_name || report.reporter_email || 'Unknown user'
}

export function BugsPage() {
  const [bugs, setBugs] = useState<BugReport[]>([])
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')
  const [updating, setUpdating] = useState(false)

  useEffect(() => {
    apiJson<{ bugs: BugReport[] }>('/v1/bug-reports')
      .then(d => {
        const next = d.bugs || []
        setBugs(next)
        setSelectedId(next[0]?.id || null)
      })
      .catch(e => setError(e.message))
      .finally(() => setLoading(false))
  }, [])

  const selected = useMemo(
    () => bugs.find(b => b.id === selectedId) || bugs[0] || null,
    [bugs, selectedId],
  )

  async function updateStatus(status: string) {
    if (!selected || selected.status === status) return
    setUpdating(true)
    try {
      const data = await apiJson<{ bug: BugReport }>(`/v1/bug-reports/${selected.id}/status`, {
        method: 'PATCH',
        body: JSON.stringify({ status }),
      })
      setBugs(prev => prev.map(b => b.id === data.bug.id ? data.bug : b))
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to update status')
    } finally {
      setUpdating(false)
    }
  }

  if (loading) return <Loading />
  if (error && !bugs.length) return <ErrorBox title="Failed to load bugs" message={error} />

  const openCount = bugs.filter(b => b.status === 'open').length
  const blockingCount = bugs.filter(b => b.severity === 'blocking').length
  const image = selected ? reportImage(selected) : null

  return (
    <>
      <div className="mb-5 flex items-end justify-between gap-4">
        <div>
          <h1 className="text-xl font-semibold tracking-tight">Bugs</h1>
          <p className="text-[12px] text-fg-4 mt-0.5">
            {bugs.length} report{bugs.length !== 1 ? 's' : ''} · {openCount} open
            {blockingCount > 0 ? ` · ${blockingCount} blocking` : ''}
          </p>
        </div>
      </div>

      {error && <ErrorBox title="Update failed" message={error} />}

      {!bugs.length ? (
        <Empty title="No bug reports yet" message="Reports submitted from the AirNote desktop app will appear here." />
      ) : (
        <div className="grid grid-cols-[minmax(0,1fr)_360px] gap-4">
          <div className="card !p-0 overflow-hidden">
            <table className="w-full">
              <thead>
                <tr>
                  {['Report', 'User', 'Severity', 'Status', 'Created'].map(h => (
                    <th key={h} className="text-[10px] font-medium text-fg-4 text-left px-5 py-3 border-b border-border uppercase tracking-wider">{h}</th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {bugs.map(bug => (
                  <tr
                    key={bug.id}
                    className={`cursor-pointer transition-colors ${selected?.id === bug.id ? 'bg-surface-4/50' : 'hover:bg-surface-4/30'}`}
                    onClick={() => setSelectedId(bug.id)}
                  >
                    <td className="px-5 py-3.5 border-b border-border-light">
                      <div className="flex items-center gap-2.5">
                        <span className="w-8 h-8 rounded-lg bg-info-bg text-info flex items-center justify-center shrink-0">
                          {reportImage(bug) ? <Camera size={15} /> : <MessageSquareText size={15} />}
                        </span>
                        <div className="min-w-0">
                          <div className="text-[13px] font-medium truncate">{bug.title}</div>
                          <div className="text-[10px] text-fg-4 truncate">
                            {bug.platform || 'unknown'}{bug.app_version ? ` · v${bug.app_version}` : ''}
                          </div>
                        </div>
                      </div>
                    </td>
                    <td className="text-[12px] text-fg-3 px-5 py-3.5 border-b border-border-light">
                      <div className="max-w-[180px] truncate">{reporter(bug)}</div>
                    </td>
                    <td className="px-5 py-3.5 border-b border-border-light"><Pill value={bug.severity} kind="severity" /></td>
                    <td className="px-5 py-3.5 border-b border-border-light"><Pill value={bug.status} kind="status" /></td>
                    <td className="text-[12px] text-fg-4 px-5 py-3.5 border-b border-border-light">{timeAgo(bug.created_at)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>

          <aside className="card h-fit sticky top-5">
            {selected ? (
              <div className="space-y-5">
                <div>
                  <div className="flex items-start gap-2">
                    <AlertTriangle size={17} className="text-warn mt-0.5 shrink-0" />
                    <div>
                      <h2 className="text-[15px] font-semibold leading-snug">{selected.title}</h2>
                      <p className="text-[11px] text-fg-4 mt-1">
                        {formatDate(selected.created_at)} · {reporter(selected)}
                      </p>
                    </div>
                  </div>
                </div>

                <div className="flex gap-2">
                  <Pill value={selected.severity} kind="severity" />
                  <Pill value={selected.status} kind="status" />
                </div>

                <div>
                  <div className="text-fg-4 text-[10px] uppercase tracking-wider mb-2">Status</div>
                  <div className="grid grid-cols-2 gap-1.5">
                    {statuses.map(status => (
                      <button
                        key={status}
                        disabled={updating}
                        onClick={() => void updateStatus(status)}
                        className={`h-8 rounded-lg text-[11px] font-semibold uppercase tracking-wide transition-colors ${
                          selected.status === status
                            ? 'bg-[hsl(0_0%_98%)] text-[hsl(240_8%_8%)]'
                            : 'bg-surface-4 text-fg-3 hover:text-fg-2'
                        }`}
                      >
                        {status}
                      </button>
                    ))}
                  </div>
                </div>

                <div>
                  <div className="text-fg-4 text-[10px] uppercase tracking-wider mb-2">Description</div>
                  <p className="text-[12.5px] leading-relaxed whitespace-pre-wrap">{selected.description}</p>
                </div>

                <div className="grid grid-cols-2 gap-2 text-[11px]">
                  <div className="rounded-lg bg-surface-4/40 px-3 py-2">
                    <div className="text-fg-4 mb-0.5">Version</div>
                    <div className="font-mono">{selected.app_version || '--'}</div>
                  </div>
                  <div className="rounded-lg bg-surface-4/40 px-3 py-2">
                    <div className="text-fg-4 mb-0.5">Platform</div>
                    <div>{selected.platform || '--'}</div>
                  </div>
                  <div className="rounded-lg bg-surface-4/40 px-3 py-2 col-span-2">
                    <div className="text-fg-4 mb-0.5">Device</div>
                    <div className="font-mono break-all">{selected.device_id || '--'}</div>
                  </div>
                </div>

                {image && (
                  <div>
                    <div className="flex items-center justify-between mb-2">
                      <div className="text-fg-4 text-[10px] uppercase tracking-wider">Screenshot</div>
                      <a href={image} target="_blank" rel="noreferrer" className="text-[11px] text-info inline-flex items-center gap-1">
                        Open <ExternalLink size={11} />
                      </a>
                    </div>
                    <a href={image} target="_blank" rel="noreferrer" className="block rounded-xl overflow-hidden border border-border bg-surface-4">
                      <img src={image} alt={selected.screenshot_name || 'Bug screenshot'} className="w-full max-h-[260px] object-contain" />
                    </a>
                  </div>
                )}

                {selected.status === 'fixed' && (
                  <div className="flex items-center gap-2 text-[11px] text-ok">
                    <CheckCircle2 size={14} /> Marked fixed
                  </div>
                )}
              </div>
            ) : (
              <div className="text-[12px] text-fg-4 py-8 text-center">Select a bug report</div>
            )}
          </aside>
        </div>
      )}
    </>
  )
}
