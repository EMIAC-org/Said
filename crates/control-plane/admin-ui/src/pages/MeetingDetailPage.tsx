import { useState, useEffect, useRef, useCallback } from 'react'
import { useParams, Link, useNavigate } from 'react-router'
import { ArrowLeft, Calendar, Clock, Users, FileText, Sparkles, Mic, ArrowRight, Radio, Globe2, Copy } from 'lucide-react'
import { LarkLogo, OpenAILogo } from '../components/BrandLogos'
import { apiJson, api } from '../api'
import { StatusPill } from '../components/StatusPill'
import { Avatar } from '../components/Avatar'
import { ErrorBox, Loading } from '../components/States'
import { formatDate, formatTime, duration, avatarColor, formatIstDateTime, formatMinutes } from '../utils'
import type { MeetingDetail } from '../types'

export function MeetingDetailPage() {
  const { id } = useParams<{ id: string }>()
  const navigate = useNavigate()
  const [data, setData] = useState<MeetingDetail | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')
  const [syncing, setSyncing] = useState(false)
  const [syncResult, setSyncResult] = useState('')
  const [actionLoading, setActionLoading] = useState('')
  const [guestLink, setGuestLink] = useState('')
  const [guestLinkLoading, setGuestLinkLoading] = useState(false)
  const [guestLinkCopied, setGuestLinkCopied] = useState(false)

  const fetchMeeting = useCallback(() =>
    apiJson<MeetingDetail>(`/v1/meetings/${id}`), [id])

  // Initial load
  useEffect(() => {
    fetchMeeting()
      .then(setData)
      .catch(e => setError(e.message))
      .finally(() => setLoading(false))
  }, [fetchMeeting])

  // Auto-poll every 5 s while the meeting is not ended
  const dataRef = useRef(data)
  dataRef.current = data

  useEffect(() => {
    // Don't start polling until initial load completes
    if (loading) return

    // If already ended, no need to poll
    if (dataRef.current?.meeting.status === 'ended') return

    const interval = setInterval(async () => {
      try {
        const fresh = await fetchMeeting()
        setData(fresh)
        // Stop polling once ended
        if (fresh.meeting.status === 'ended') clearInterval(interval)
      } catch {
        // Silently ignore transient poll errors — initial data still shown
      }
    }, 5000)

    return () => clearInterval(interval)
  }, [fetchMeeting, loading])

  if (loading) return <Loading />
  if (error || !data) return <ErrorBox title="Failed to load" message={error || 'Not found'} />

  const { meeting: m, participants, summary, tasks, decisions, transcript } = data
  const pNames = participants.map((p, i) => p.lark_name || p.name || `Participant ${i + 1}`)
  const words = transcript.reduce((s, c) => s + (c.text || '').split(/\s+/).length, 0)

  const doAction = async (action: 'start' | 'end') => {
    if (action === 'end' && !confirm('End this meeting?')) return
    setActionLoading(action)
    try {
      await apiJson(`/v1/meetings/${id}/${action}`, { method: 'POST' })
      const fresh = await apiJson<MeetingDetail>(`/v1/meetings/${id}`)
      setData(fresh)
    } catch (e) { alert('Failed: ' + (e as Error).message) }
    setActionLoading('')
  }

  const doSync = async () => {
    setSyncing(true)
    try {
      const d = await apiJson<{ tasks_synced?: number; doc_id?: string; messages_sent?: number }>(`/v1/meetings/${id}/sync-to-lark`, { method: 'POST' })
      setSyncResult(`Synced: ${d.tasks_synced ?? 0} tasks${d.doc_id ? ' · Doc created' : ''}${d.messages_sent ? ` · ${d.messages_sent} messages` : ''}`)
    } catch (e) { setSyncResult('Failed: ' + (e as Error).message) }
    setSyncing(false)
  }

  const generateGuestLink = async () => {
    setGuestLinkLoading(true)
    setGuestLinkCopied(false)
    try {
      const result = await apiJson<{ guest_link: string }>(`/v1/meetings/${id}/guest-link`, { method: 'POST' })
      setGuestLink(new URL(result.guest_link, window.location.origin).toString())
    } catch (e) {
      alert('Failed: ' + (e as Error).message)
    } finally {
      setGuestLinkLoading(false)
    }
  }

  const copyGuestLink = async () => {
    if (!guestLink) return
    await navigator.clipboard.writeText(guestLink)
    setGuestLinkCopied(true)
    window.setTimeout(() => setGuestLinkCopied(false), 1600)
  }

  return (
    <>
      <Link to="/meetings" className="inline-flex items-center gap-1.5 text-xs text-fg-4 hover:text-fg mb-5 transition-colors">
        <ArrowLeft size={14} /> Meetings
      </Link>

      {/* Header card */}
      <div className="card mb-4">
        <div className="flex items-start justify-between mb-3">
          <h1 className="text-xl font-semibold tracking-tight">{m.title}</h1>
          <StatusPill status={m.status} />
        </div>
        <div className="flex flex-wrap items-center gap-4 text-[12px] text-fg-3">
          <span className="flex items-center gap-1.5"><Calendar size={13} className="text-fg-4" /> {m.scheduled_at ? formatIstDateTime(m.scheduled_at) : formatDate(m.created_at)}</span>
          {m.scheduled_at && <span className="flex items-center gap-1.5"><Clock size={13} className="text-fg-4" /> {formatMinutes(m.duration_minutes)}</span>}
          {m.started_at && <span className="flex items-center gap-1.5"><Clock size={13} className="text-fg-4" /> {duration(m.started_at, m.ended_at || new Date().toISOString())}</span>}
          {m.lark_event_status && <span className="flex items-center gap-1.5"><LarkLogo className="w-3.5 h-3.5" /> Calendar {m.lark_event_status}</span>}
          <span className="flex items-center gap-1.5"><Users size={13} className="text-fg-4" /> {participants.length} participants</span>
          {words > 0 && <span className="flex items-center gap-1.5"><FileText size={13} className="text-fg-4" /> {words.toLocaleString()} words</span>}
        </div>

        {/* Actions */}
        {m.status === 'scheduled' && (
          <div className="mt-4 pt-4 border-t border-border-light flex items-center gap-3">
            <button onClick={() => doAction('start')} disabled={!!actionLoading} className="inline-flex items-center gap-1.5 text-[13px] font-semibold px-5 h-10 rounded-lg bg-[hsl(0_0%_98%)] text-[hsl(240_8%_8%)] hover:opacity-90 disabled:opacity-35 transition-all">
              {actionLoading === 'start' && <div className="spinner" style={{ width: 14, height: 14, borderWidth: 2 }} />}
              {actionLoading === 'start' ? 'Starting...' : 'Start Meeting'}
            </button>
            <button onClick={() => navigate(`/meetings/${id}/live`)} className="inline-flex items-center gap-1.5 text-[13px] font-semibold px-5 h-10 rounded-lg bg-[hsl(0_0%_98%)] text-[hsl(240_8%_8%)] hover:opacity-90 transition-all">
              <Radio size={14} /> Join Live
            </button>
          </div>
        )}
        {m.status === 'live' && (
          <div className="mt-4 pt-4 border-t border-border-light flex items-center gap-3">
            <button onClick={() => navigate(`/meetings/${id}/live`)} className="inline-flex items-center gap-1.5 text-[13px] font-semibold px-5 h-10 rounded-lg bg-[hsl(0_0%_98%)] text-[hsl(240_8%_8%)] hover:opacity-90 transition-all">
              <Radio size={14} /> Join Live
            </button>
            <button onClick={() => doAction('end')} disabled={!!actionLoading} className="inline-flex items-center gap-1.5 text-[13px] font-semibold px-5 h-10 rounded-lg text-live border border-live/30 hover:bg-live-bg disabled:opacity-35 transition-all">
              {actionLoading === 'end' && <div className="spinner" style={{ width: 14, height: 14, borderWidth: 2 }} />}
              {actionLoading === 'end' ? 'Ending...' : 'End Meeting'}
            </button>
          </div>
        )}
      </div>

      <div className="grid grid-cols-[2fr_1fr] gap-4">
        {/* Left column */}
        <div className="space-y-4">
          {/* Summary */}
          {summary && (
            <div className="card">
              <div className="text-[12px] font-semibold flex items-center gap-2 mb-3">
                <OpenAILogo size={15} className="text-fg-4" /> AI Summary
              </div>
              <p className="text-[13px] text-fg-2 leading-relaxed">{summary}</p>
            </div>
          )}

          {/* Tasks */}
          {tasks.length > 0 && (
            <div className="card">
              <div className="text-[12px] font-semibold mb-3">Action Items</div>
              <div className="flex flex-col gap-2">
                {tasks.map(t => (
                  <div key={t.id} className="flex items-center gap-3 px-3.5 py-2.5 bg-surface-2 rounded-lg">
                    <div className={`w-[18px] h-[18px] rounded-md border-[1.5px] shrink-0 ${t.lark_task_id ? 'bg-ok border-ok' : 'border-fg-5'}`} />
                    <div className="flex-1 text-[13px]">{t.title}</div>
                    <div className="flex items-center gap-1.5 text-xs text-fg-3">
                      {t.assignee ? <><Avatar name={t.assignee} size="sm" /> {t.assignee}</> : <span className="text-fg-4">Unassigned</span>}
                    </div>
                    {t.lark_task_id && <span className="text-[10px] font-semibold px-2 py-0.5 rounded-full bg-ok-bg text-ok">Synced</span>}
                  </div>
                ))}
              </div>
            </div>
          )}

          {/* Decisions */}
          {decisions.length > 0 && (
            <div className="card">
              <div className="text-[12px] font-semibold mb-3">Decisions</div>
              <div className="space-y-2">
                {decisions.map(d => (
                  <div key={d.id} className="flex items-start gap-3 px-3.5 py-2.5 bg-surface-2 rounded-lg">
                    <div className="w-1.5 h-1.5 rounded-full bg-warn mt-2 shrink-0" />
                    <div className="text-[13px] leading-relaxed">{d.text}</div>
                  </div>
                ))}
              </div>
            </div>
          )}

          {/* Transcript */}
          {transcript.length > 0 && (
            <div className="card !p-0 overflow-hidden">
              <div className="px-5 py-3.5 flex items-center gap-2">
                <Mic size={13} className="text-fg-4" />
                <span className="text-[12px] font-semibold">Transcript</span>
                <span className="ml-auto text-[10px] text-fg-4">{transcript.length} chunks &middot; {words.toLocaleString()} words</span>
              </div>
              <div className="max-h-[400px] overflow-y-auto px-5 py-3 border-t border-border-light">
                {transcript.map((c, i) => {
                  const sp = c.speaker_name || 'Unknown'
                  return (
                    <div key={i} className={`flex gap-3 py-2 ${i > 0 ? 'border-t border-border-light' : ''}`}>
                      <div className="text-xs font-semibold w-[90px] shrink-0 pt-px" style={{ color: avatarColor(sp) }}>{sp}</div>
                      <div className="text-[13px] text-fg-2 leading-relaxed flex-1">{c.text}</div>
                    </div>
                  )
                })}
              </div>
            </div>
          )}
        </div>

        {/* Right column */}
        <div className="space-y-4">
          {/* Guest browser link */}
          {(m.status === 'scheduled' || m.status === 'live') && (
            <div className="card">
              <div className="flex items-center gap-3 mb-3">
                <div className="w-9 h-9 rounded-lg bg-surface-4 flex items-center justify-center">
                  <Globe2 size={18} className="text-fg-3" />
                </div>
                <div className="flex-1">
                  <div className="text-[12px] font-semibold">Guest Browser Link</div>
                  <div className="text-[10px] text-fg-4">Share with guests who do not have AirNote installed</div>
                </div>
              </div>
              {!guestLink ? (
                <button onClick={generateGuestLink} disabled={guestLinkLoading} className="w-full inline-flex items-center justify-center gap-1.5 text-xs font-semibold px-4 h-9 rounded-lg bg-[hsl(0_0%_98%)] text-[hsl(240_8%_8%)] hover:opacity-90 disabled:opacity-35 transition-all">
                  {guestLinkLoading ? <div className="spinner" style={{ width: 14, height: 14, borderWidth: 2 }} /> : <Globe2 size={13} />}
                  {guestLinkLoading ? 'Generating...' : 'Generate Guest Link'}
                </button>
              ) : (
                <div className="space-y-2">
                  <div className="px-3 py-2 rounded-lg bg-surface-2 border border-border-light text-[11px] text-fg-3 break-all">{guestLink}</div>
                  <button onClick={copyGuestLink} className="w-full inline-flex items-center justify-center gap-1.5 text-xs font-semibold px-4 h-9 rounded-lg bg-surface-3 text-fg border border-border-light hover:bg-surface-4 transition-all">
                    <Copy size={13} /> {guestLinkCopied ? 'Copied' : 'Copy'}
                  </button>
                </div>
              )}
            </div>
          )}

          {/* Agenda */}
          {m.agenda && (
            <div className="card">
              <div className="text-[12px] font-semibold flex items-center gap-2 mb-3">
                <FileText size={13} className="text-fg-4" /> Agenda
              </div>
              <p className="text-[13px] text-fg-2 leading-relaxed whitespace-pre-wrap">{m.agenda}</p>
            </div>
          )}

          {/* Participants */}
          <div className="card">
            <div className="text-[12px] font-semibold flex items-center gap-2 mb-3">
              <Users size={13} className="text-fg-4" /> Participants
            </div>
            {!participants.length ? <div className="text-center py-4 text-fg-4 text-[12px]">No participants.</div> : (
              <div className="space-y-1.5">
                {participants.map((p, i) => {
                  const name = pNames[i], st = p.status || 'invited'
                  const cls = st === 'connected' || st === 'joined' ? 'text-ok' : st === 'left' || st === 'disconnected' ? 'text-warn' : 'text-fg-4'
                  return (
                    <div key={p.id} className="flex items-center gap-2.5 px-3 py-2 bg-surface-2 rounded-lg">
                      <Avatar name={name} size="sm" />
                      <div className="flex-1 min-w-0">
                        <div className="text-[12px] font-medium truncate">{name}</div>
                        {p.joined_at && <div className="text-[10px] text-fg-4">Joined {formatTime(p.joined_at)}</div>}
                      </div>
                      <span className={`text-[10px] font-medium ${cls}`}>{st.charAt(0).toUpperCase() + st.slice(1)}</span>
                    </div>
                  )
                })}
              </div>
            )}
          </div>

          {/* AI pending */}
          {!summary && transcript.length > 0 && m.status === 'ended' && (
            <div className="bg-warn-bg border border-warn/20 rounded-xl px-4 py-3.5">
              <div className="text-[12px] font-semibold text-warn mb-0.5">AI Summary Pending</div>
              <div className="text-[11px] text-fg-3">Connect OpenAI in Settings to enable AI processing.</div>
            </div>
          )}

          {/* Lark sync */}
          {m.status === 'ended' && (
            <div className="card">
              <div className="flex items-center gap-3 mb-3">
                <div className="w-9 h-9 rounded-lg bg-surface-4 flex items-center justify-center">
                  <LarkLogo size={20} />
                </div>
                <div className="flex-1">
                  <div className="text-[12px] font-semibold">Sync to Lark</div>
                  <div className="text-[10px] text-fg-4">{tasks.length} task{tasks.length !== 1 ? 's' : ''} + summary</div>
                </div>
              </div>
              <button onClick={doSync} disabled={syncing} className="w-full inline-flex items-center justify-center gap-1.5 text-xs font-semibold px-4 h-9 rounded-lg bg-[hsl(0_0%_98%)] text-[hsl(240_8%_8%)] hover:opacity-90 disabled:opacity-35 transition-all">
                {syncing ? <div className="spinner" style={{ width: 14, height: 14, borderWidth: 2 }} /> : null}
                {syncing ? 'Syncing...' : <>Sync All <ArrowRight size={12} /></>}
              </button>
              {syncResult && <div className={`mt-2 rounded-lg px-3 py-2 text-[11px] ${syncResult.startsWith('Failed') ? 'bg-live-bg text-live' : 'bg-ok-bg text-ok'}`}>{syncResult}</div>}
            </div>
          )}
        </div>
      </div>

      {/* No data */}
      {!summary && !tasks.length && !decisions.length && !transcript.length && (
        <div className="card text-center py-10 mt-4">
          <Sparkles size={20} className="text-fg-4 mx-auto mb-2" />
          <h3 className="text-[14px] font-semibold text-fg mb-1">No meeting data yet</h3>
          <p className="text-[12px] text-fg-3">Transcript and AI output will appear once the meeting has activity.</p>
        </div>
      )}
    </>
  )
}
