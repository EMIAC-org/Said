import { useState, useEffect } from 'react'
import { apiJson } from '../api'
import { useAuth } from '../hooks/useAuth'
import { Avatar } from '../components/Avatar'
import { Loading, Empty, ErrorBox } from '../components/States'
import { formatDate } from '../utils'
import type { DesktopClient } from '../types'

interface DesktopVocabDetail {
  terms: Array<{ term: string; term_type: string; weight: number; use_count: number; safety_status: string }>
  aliases: Array<{ transcript_form: string; correct_form: string; weight: number; use_count: number; review_status: string; safety_status: string }>
}

function relativeLastSeen(iso: string): string {
  const ms = Date.now() - new Date(iso).getTime()
  const min = Math.floor(ms / 60_000)
  if (min < 1) return 'Just now'
  if (min < 60) return `${min}m ago`
  const hr = Math.floor(min / 60)
  if (hr < 24) return `${hr}h ago`
  return `${Math.floor(hr / 24)}d ago`
}

function isActive(lastSeen: string): boolean {
  return Date.now() - new Date(lastSeen).getTime() < 15 * 60 * 1000
}

export function DesktopPage() {
  const { org } = useAuth()
  const [clients, setClients] = useState<DesktopClient[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')
  const [selected, setSelected] = useState<DesktopClient | null>(null)
  const [usage, setUsage] = useState<{ polish_count: number; word_count: number } | null>(null)
  const [vocab, setVocab] = useState<DesktopVocabDetail | null>(null)

  useEffect(() => {
    if (!org?.org?.id) { setLoading(false); return }
    apiJson<{ clients: DesktopClient[] }>(`/v1/orgs/${org.org.id}/clients`)
      .then(d => setClients(d.clients || []))
      .catch(e => setError(e.message))
      .finally(() => setLoading(false))
  }, [org])

  useEffect(() => {
    if (!selected || !org?.org?.id) {
      setUsage(null)
      setVocab(null)
      return
    }
    apiJson<{ usage: { polish_count: number; word_count: number } }>(
      `/v1/orgs/${org.org.id}/clients/${selected.account_id}/usage`,
    )
      .then(d => setUsage(d.usage))
      .catch(() => setUsage(null))
    apiJson<DesktopVocabDetail>(
      `/v1/orgs/${org.org.id}/clients/${selected.account_id}/vocab`,
    )
      .then(d => setVocab(d))
      .catch(() => setVocab(null))
  }, [selected, org])

  if (loading) return <Loading />
  if (error) return <ErrorBox title="Failed to load" message={error} />

  const orgName = org?.org?.name || ''
  const activeCount = clients.filter(c => isActive(c.last_seen_at)).length

  return (
    <>
      <div className="mb-5 flex items-end justify-between gap-4">
        <div>
          <h1 className="text-xl font-semibold tracking-tight">Desktop</h1>
          <p className="text-[12px] text-fg-4 mt-0.5">
            {orgName}{orgName ? ' · ' : ''}{clients.length} install{clients.length !== 1 ? 's' : ''}
            {activeCount > 0 ? ` · ${activeCount} active now` : ''}
          </p>
        </div>
      </div>

      {!clients.length ? (
        <Empty title="No desktop installs yet" message="Users will appear here after they connect the AirNote desktop app to this workspace." />
      ) : (
        <div className="grid grid-cols-[1fr_280px] gap-4">
          <div className="card !p-0 overflow-hidden">
            <table className="w-full">
              <thead>
                <tr>
                  {['User', 'Platform', 'Version', 'Last seen', 'Status'].map(h => (
                    <th key={h} className="text-[10px] font-medium text-fg-4 text-left px-5 py-3 border-b border-border uppercase tracking-wider">{h}</th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {clients.map(c => {
                  const name = c.lark_name || c.email || `User ${c.account_id.substring(0, 8)}`
                  const active = isActive(c.last_seen_at)
                  return (
                    <tr
                      key={c.id}
                      className={`cursor-pointer transition-colors ${selected?.id === c.id ? 'bg-surface-4/50' : 'hover:bg-surface-4/30'}`}
                      onClick={() => setSelected(c)}
                    >
                      <td className="px-5 py-3.5 border-b border-border-light">
                        <div className="flex items-center gap-2.5">
                          <Avatar name={name} size="sm" />
                          <div>
                            <div className="text-[13px] font-medium">{name}</div>
                            {c.email && c.lark_name && (
                              <div className="text-[10px] text-fg-4">{c.email}</div>
                            )}
                          </div>
                        </div>
                      </td>
                      <td className="text-[12px] text-fg-3 px-5 py-3.5 border-b border-border-light capitalize">{c.platform}</td>
                      <td className="text-[12px] text-fg-3 px-5 py-3.5 border-b border-border-light font-mono">{c.app_version}</td>
                      <td className="text-[12px] text-fg-3 px-5 py-3.5 border-b border-border-light">{relativeLastSeen(c.last_seen_at)}</td>
                      <td className="px-5 py-3.5 border-b border-border-light">
                        <span
                          className="text-[10px] font-semibold px-2 py-0.5 rounded-full"
                          style={{
                            background: active ? 'hsl(145 60% 16%)' : 'hsl(var(--surface-4))',
                            color: active ? 'hsl(145 70% 65%)' : 'hsl(var(--fg-4))',
                          }}
                        >
                          {active ? 'Active' : 'Offline'}
                        </span>
                      </td>
                    </tr>
                  )
                })}
              </tbody>
            </table>
          </div>

          <div className="card">
            {selected ? (
              <>
                <div className="flex items-center gap-3 mb-4">
                  <Avatar name={selected.lark_name || selected.email || '?'} size="lg" />
                  <div>
                    <div className="text-[14px] font-semibold">{selected.lark_name || selected.email}</div>
                    <div className="text-[11px] text-fg-4">{selected.platform} · v{selected.app_version}</div>
                  </div>
                </div>
                <div className="space-y-3 text-[12px]">
                  <div className="grid grid-cols-2 gap-2">
                    <div className="rounded-lg bg-surface-4/40 px-3 py-2">
                      <div className="text-[18px] font-semibold tabular-nums">{selected.company_bucket_version || 0}</div>
                      <div className="text-[10px] text-fg-4">Bucket v</div>
                    </div>
                    <div className="rounded-lg bg-surface-4/40 px-3 py-2">
                      <div className="text-[18px] font-semibold tabular-nums">{(selected.personal_vocab_count || 0) + (selected.personal_alias_count || 0)}</div>
                      <div className="text-[10px] text-fg-4">Vocab rows</div>
                    </div>
                  </div>
                  <div>
                    <div className="text-fg-4 text-[10px] uppercase tracking-wider mb-1">First seen</div>
                    <div>{formatDate(selected.first_seen_at)}</div>
                  </div>
                  <div>
                    <div className="text-fg-4 text-[10px] uppercase tracking-wider mb-1">Last seen</div>
                    <div>{formatDate(selected.last_seen_at)}</div>
                  </div>
                  {selected.hostname && (
                    <div>
                      <div className="text-fg-4 text-[10px] uppercase tracking-wider mb-1">Hostname</div>
                      <div className="font-mono text-[11px]">{selected.hostname}</div>
                    </div>
                  )}
                  {selected.company_vocab_synced_at && (
                    <div>
                      <div className="text-fg-4 text-[10px] uppercase tracking-wider mb-1">Vocab synced</div>
                      <div>{formatDate(selected.company_vocab_synced_at)}</div>
                    </div>
                  )}
                  <div className="pt-3 border-t border-border-light">
                    <div className="text-fg-4 text-[10px] uppercase tracking-wider mb-2">7-day usage</div>
                    {usage ? (
                      <div className="grid grid-cols-2 gap-2">
                        <div className="rounded-lg bg-surface-4/40 px-3 py-2">
                          <div className="text-[18px] font-semibold tabular-nums">{usage.polish_count}</div>
                          <div className="text-[10px] text-fg-4">Polishes</div>
                        </div>
                        <div className="rounded-lg bg-surface-4/40 px-3 py-2">
                          <div className="text-[18px] font-semibold tabular-nums">{usage.word_count.toLocaleString()}</div>
                          <div className="text-[10px] text-fg-4">Words</div>
                        </div>
                      </div>
                    ) : (
                      <div className="text-fg-4">No usage recorded yet</div>
                    )}
                  </div>
                  <div className="pt-3 border-t border-border-light">
                    <div className="text-fg-4 text-[10px] uppercase tracking-wider mb-2">Uploaded vocabulary</div>
                    {vocab && (vocab.terms.length || vocab.aliases.length) ? (
                      <div className="space-y-3">
                        <div>
                          <div className="text-[11px] font-medium mb-1">Terms</div>
                          <div className="space-y-1 max-h-[130px] overflow-y-auto">
                            {vocab.terms.slice(0, 12).map(t => (
                              <div key={t.term} className="flex items-center justify-between gap-2 text-[11px] rounded-md bg-surface-4/35 px-2 py-1">
                                <span className="truncate">{t.term}</span>
                                <span className="text-fg-4 tabular-nums">{t.use_count}</span>
                              </div>
                            ))}
                          </div>
                        </div>
                        <div>
                          <div className="text-[11px] font-medium mb-1">Aliases</div>
                          <div className="space-y-1 max-h-[130px] overflow-y-auto">
                            {vocab.aliases.slice(0, 12).map(a => (
                              <div key={`${a.transcript_form}-${a.correct_form}`} className="text-[11px] rounded-md bg-surface-4/35 px-2 py-1">
                                <span className="font-mono">{a.transcript_form}</span>
                                <span className="text-fg-4"> → </span>
                                <span>{a.correct_form}</span>
                              </div>
                            ))}
                          </div>
                        </div>
                      </div>
                    ) : (
                      <div className="text-fg-4">No vocabulary summary uploaded yet</div>
                    )}
                  </div>
                </div>
              </>
            ) : (
              <div className="text-[12px] text-fg-4 py-8 text-center">Select an install to view details</div>
            )}
          </div>
        </div>
      )}
    </>
  )
}
