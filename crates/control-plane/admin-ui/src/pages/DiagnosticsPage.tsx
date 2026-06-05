import { useEffect, useMemo, useState } from 'react'
import { AlertTriangle, ServerCrash } from 'lucide-react'
import { apiJson } from '../api'
import { Empty, ErrorBox, Loading } from '../components/States'
import { formatDate, timeAgo } from '../utils'
import type { DiagnosticsEvent } from '../types'

const severityClasses: Record<string, string> = {
  info: 'text-info bg-info-bg',
  warning: 'text-warn bg-warn-bg',
  error: 'text-live bg-live-bg',
  fatal: 'text-live bg-live-bg',
}

function Pill({ value }: { value: string }) {
  const cls = severityClasses[value] || 'text-fg-3 bg-surface-4'
  return (
    <span className={`inline-flex items-center text-[10px] font-semibold px-2.5 py-1 rounded-full uppercase tracking-wide ${cls}`}>
      {value}
    </span>
  )
}

const SEVERITIES = ['all', 'fatal', 'error', 'warning', 'info'] as const

export function DiagnosticsPage() {
  const [events, setEvents] = useState<DiagnosticsEvent[]>([])
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')
  const [severity, setSeverity] = useState<string>('all')
  const [typeQuery, setTypeQuery] = useState('')

  useEffect(() => {
    apiJson<{ events: DiagnosticsEvent[] }>('/v1/diagnostics')
      .then(d => {
        const next = d.events || []
        setEvents(next)
        setSelectedId(next[0]?.id || null)
      })
      .catch(e => setError(e.message))
      .finally(() => setLoading(false))
  }, [])

  // Client-side filter over the (max 500) fetched events. Severity is exact;
  // type is a case-insensitive substring so "panic" matches panic + panic.recovered.
  const filtered = useMemo(() => {
    const q = typeQuery.trim().toLowerCase()
    return events.filter(e => {
      if (severity !== 'all' && e.severity !== severity) return false
      if (q && !e.event_type.toLowerCase().includes(q)) return false
      return true
    })
  }, [events, severity, typeQuery])

  const selected = useMemo(
    () => filtered.find(e => e.id === selectedId) || filtered[0] || null,
    [filtered, selectedId],
  )

  // Distinct event types present, for quick one-click filtering.
  const typeChips = useMemo(() => {
    const counts = new Map<string, number>()
    for (const e of events) counts.set(e.event_type, (counts.get(e.event_type) || 0) + 1)
    return [...counts.entries()].sort((a, b) => b[1] - a[1]).slice(0, 8)
  }, [events])

  if (loading) return <Loading />
  if (error) return <ErrorBox title="Failed to load diagnostics" message={error} />

  return (
    <div className="flex flex-col gap-5 h-full min-h-0">
      <div>
        <h1 className="text-[22px] font-semibold tracking-tight text-fg">Fleet diagnostics</h1>
        <p className="text-[13px] text-fg-3 mt-1">
          Anonymous crash and error events from desktop installs. No transcript or polished text is stored.
        </p>
      </div>

      {events.length > 0 && (
        <div className="flex flex-wrap items-center gap-3">
          {/* Severity segmented control */}
          <div className="inline-flex items-center rounded-lg border border-border bg-surface-2 p-0.5">
            {SEVERITIES.map(s => (
              <button
                key={s}
                onClick={() => setSeverity(s)}
                className={`px-2.5 py-1 text-[11px] font-medium rounded-md capitalize transition-colors ${
                  severity === s ? 'bg-surface-4 text-fg' : 'text-fg-4 hover:text-fg-2'
                }`}
              >
                {s}
              </button>
            ))}
          </div>

          {/* Event-type substring filter */}
          <input
            value={typeQuery}
            onChange={e => setTypeQuery(e.target.value)}
            placeholder="filter type… e.g. panic, state.healed"
            className="text-[12px] px-3 py-1.5 rounded-lg border border-border bg-surface-2 text-fg placeholder:text-fg-4 w-[260px] focus:outline-none focus:border-border-light"
          />

          {/* Quick chips for the most common types */}
          {typeChips.map(([type, count]) => (
            <button
              key={type}
              onClick={() => setTypeQuery(typeQuery === type ? '' : type)}
              className={`px-2.5 py-1 text-[11px] font-mono rounded-full border transition-colors ${
                typeQuery === type
                  ? 'border-border-light bg-surface-4 text-fg'
                  : 'border-border bg-surface-2 text-fg-3 hover:text-fg'
              }`}
            >
              {type} <span className="text-fg-4">{count}</span>
            </button>
          ))}

          {(severity !== 'all' || typeQuery) && (
            <button
              onClick={() => { setSeverity('all'); setTypeQuery('') }}
              className="text-[11px] text-fg-4 hover:text-fg underline underline-offset-2"
            >
              clear
            </button>
          )}
        </div>
      )}

      {events.length === 0 ? (
        <Empty
          title="No diagnostics yet"
          message="Events appear here when shipped clients POST to /v1/diagnostics."
        />
      ) : (
        <div className="grid grid-cols-[minmax(0,1.1fr)_minmax(0,0.9fr)] gap-4 min-h-0 flex-1">
          <div className="rounded-xl border border-border bg-surface-2 overflow-hidden flex flex-col min-h-0">
            <div className="px-4 py-3 border-b border-border text-[12px] font-medium text-fg-3">
              Recent events ({filtered.length}{filtered.length !== events.length ? ` of ${events.length}` : ''})
            </div>
            <div className="overflow-auto flex-1">
              {filtered.length === 0 ? (
                <div className="px-4 py-8 text-[12px] text-fg-4 text-center">
                  No events match this filter.
                </div>
              ) : (
              <table className="w-full text-left text-[12px]">
                <thead className="sticky top-0 bg-surface-3 text-fg-4 uppercase tracking-wide text-[10px]">
                  <tr>
                    <th className="px-4 py-2 font-medium">When</th>
                    <th className="px-4 py-2 font-medium">Severity</th>
                    <th className="px-4 py-2 font-medium">Type</th>
                    <th className="px-4 py-2 font-medium">Device</th>
                  </tr>
                </thead>
                <tbody>
                  {filtered.map(event => (
                    <tr
                      key={event.id}
                      onClick={() => setSelectedId(event.id)}
                      className={`border-t border-border/60 cursor-pointer hover:bg-surface-3/50 ${
                        selected?.id === event.id ? 'bg-surface-3/80' : ''
                      }`}
                    >
                      <td className="px-4 py-2.5 text-fg-3 whitespace-nowrap">{timeAgo(event.created_at)}</td>
                      <td className="px-4 py-2.5"><Pill value={event.severity} /></td>
                      <td className="px-4 py-2.5 font-mono text-[11px] text-fg-2 max-w-[220px] truncate">{event.event_type}</td>
                      <td className="px-4 py-2.5 font-mono text-[11px] text-fg-4 max-w-[120px] truncate">{event.device_id}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
              )}
            </div>
          </div>

          <div className="rounded-xl border border-border bg-surface-2 p-5 overflow-auto min-h-0">
            {selected ? (
              <div className="flex flex-col gap-4">
                <div className="flex items-start gap-3">
                  <div className="p-2 rounded-lg bg-surface-4 text-fg-3">
                    {selected.severity === 'fatal' ? <ServerCrash size={18} /> : <AlertTriangle size={18} />}
                  </div>
                  <div>
                    <div className="text-[15px] font-semibold text-fg font-mono">{selected.event_type}</div>
                    <div className="text-[12px] text-fg-3 mt-1">
                      {formatDate(selected.created_at)} · {selected.app_version || 'unknown version'}
                    </div>
                  </div>
                </div>
                <div className="grid grid-cols-2 gap-3 text-[12px]">
                  <div><span className="text-fg-4">OS</span><div className="text-fg">{selected.os || '—'}</div></div>
                  <div><span className="text-fg-4">Arch</span><div className="text-fg">{selected.arch || '—'}</div></div>
                  <div><span className="text-fg-4">Channel</span><div className="text-fg">{selected.channel || '—'}</div></div>
                  <div><span className="text-fg-4">Phase</span><div className="text-fg">{selected.phase || '—'}</div></div>
                </div>
                <div>
                  <div className="text-[11px] uppercase tracking-wide text-fg-4 mb-2">Context</div>
                  <pre className="text-[11px] leading-relaxed bg-surface-4/60 rounded-lg p-3 overflow-auto text-fg-2">
                    {JSON.stringify(selected.context, null, 2)}
                  </pre>
                </div>
              </div>
            ) : null}
          </div>
        </div>
      )}
    </div>
  )
}
