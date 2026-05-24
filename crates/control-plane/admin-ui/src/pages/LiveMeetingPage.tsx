import { useState, useEffect, useRef, useCallback } from 'react'
import { useParams, Link } from 'react-router'
import { ArrowLeft, Mic, CheckSquare, Sparkles, Send, Check, Clock } from 'lucide-react'
import { apiJson, getToken } from '../api'
import { useAuth } from '../hooks/useAuth'
import { useWebSocket, type WsMessage, type TranscriptChunk, type WsTask, type WsParticipant } from '../hooks/useWebSocket'
import { Avatar } from '../components/Avatar'
import { Loading, ErrorBox } from '../components/States'

const SPEAKER_COLORS = [
  'var(--color-speaker-1)',
  'var(--color-speaker-2)',
  'var(--color-speaker-3)',
  'var(--color-speaker-4)',
  'var(--color-speaker-5)',
  'var(--color-speaker-6)',
]

function speakerColor(name: string): string {
  let h = 0
  for (let i = 0; i < name.length; i++) h = name.charCodeAt(i) + ((h << 5) - h)
  return SPEAKER_COLORS[Math.abs(h) % SPEAKER_COLORS.length]
}

interface TaskItem extends WsTask {
  selected: boolean
  pushing: boolean
  synced: boolean
}

export function LiveMeetingPage() {
  const { id } = useParams<{ id: string }>()
  const { user } = useAuth()
  const token = getToken() || ''

  const [meetingTitle, setMeetingTitle] = useState('')
  const [meetingStatus, setMeetingStatus] = useState('')
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')

  const [chunks, setChunks] = useState<TranscriptChunk[]>([])
  const [tasks, setTasks] = useState<TaskItem[]>([])
  const [decisions, setDecisions] = useState<string[]>([])
  const [summary, setSummary] = useState<string | null>(null)
  const [participants, setParticipants] = useState<WsParticipant[]>([])
  const [newTaskFlash, setNewTaskFlash] = useState<string | null>(null)

  const transcriptEndRef = useRef<HTMLDivElement>(null)
  const taskFlashTimeout = useRef<ReturnType<typeof setTimeout> | null>(null)

  // Fetch meeting info
  useEffect(() => {
    if (!id) return
    apiJson<{ meeting: { title: string; status: string } }>(`/v1/meetings/${id}`)
      .then(d => {
        setMeetingTitle(d.meeting.title)
        setMeetingStatus(d.meeting.status)
      })
      .catch(e => setError(e.message))
      .finally(() => setLoading(false))
  }, [id])

  const onMessage = useCallback((msg: WsMessage) => {
    switch (msg.type) {
      case 'catchup':
        setChunks(msg.current_chunks || [])
        setSummary(msg.summary)
        setDecisions((msg.decisions || []).map(d => d.text))
        setParticipants(msg.participants || [])
        setTasks((msg.tasks || []).map(t => ({
          ...t,
          selected: false,
          pushing: false,
          synced: !!(t as TaskItem).lark_task_id,
        })))
        break

      case 'transcript_chunk':
        setChunks(prev => [...prev, msg])
        setTimeout(() => transcriptEndRef.current?.scrollIntoView({ behavior: 'smooth' }), 50)
        break

      case 'task_detected':
        setTasks(prev => {
          if (prev.some(t => t.task_id === msg.task_id)) return prev
          return [...prev, { ...msg, selected: false, pushing: false, synced: false }]
        })
        setNewTaskFlash(msg.title)
        if (taskFlashTimeout.current) clearTimeout(taskFlashTimeout.current)
        taskFlashTimeout.current = setTimeout(() => setNewTaskFlash(null), 4000)
        break

      case 'summary_updated':
        setSummary(msg.summary)
        break

      case 'participant_joined':
        setParticipants(prev => {
          if (prev.some(p => p.account_id === msg.account_id)) {
            return prev.map(p => p.account_id === msg.account_id ? { ...p, status: 'joined' } : p)
          }
          return [...prev, { account_id: msg.account_id, email: msg.email, status: 'joined' }]
        })
        break

      case 'participant_left':
        setParticipants(prev => prev.map(p => p.account_id === msg.account_id ? { ...p, status: 'left' } : p))
        break

      case 'meeting_ended':
        setMeetingStatus('ended')
        break
    }
  }, [])

  const { connected } = useWebSocket({ meetingId: id || '', token, onMessage })

  const toggleTask = (taskId: string) => {
    setTasks(prev => prev.map(t => t.task_id === taskId && !t.synced ? { ...t, selected: !t.selected } : t))
  }

  const selectAll = () => {
    setTasks(prev => prev.map(t => t.synced ? t : { ...t, selected: true }))
  }

  const pushSelected = async () => {
    const toPush = tasks.filter(t => t.selected && !t.synced)
    if (toPush.length === 0) return

    setTasks(prev => prev.map(t => t.selected && !t.synced ? { ...t, pushing: true } : t))

    try {
      await apiJson(`/v1/meetings/${id}/push-tasks`, {
        method: 'POST',
        body: JSON.stringify({ task_ids: toPush.map(t => t.task_id) }),
      })
      setTasks(prev => prev.map(t => {
        if (toPush.some(p => p.task_id === t.task_id)) {
          return { ...t, synced: true, pushing: false, selected: false }
        }
        return t
      }))
    } catch (e) {
      alert('Push failed: ' + (e as Error).message)
      setTasks(prev => prev.map(t => ({ ...t, pushing: false })))
    }
  }

  if (loading) return <Loading />
  if (error) return <ErrorBox title="Failed to load" message={error} />

  const draftTasks = tasks.filter(t => !t.synced)
  const syncedTasks = tasks.filter(t => t.synced)
  const selectedCount = draftTasks.filter(t => t.selected).length
  const onlineCount = participants.filter(p => p.status === 'joined' || p.status === 'connected').length

  return (
    <div className="flex flex-col h-[calc(100vh-80px)] -mx-10 -my-7">
      {/* Header bar */}
      <div className="flex items-center justify-between px-6 py-3 border-b border-surface-4 shrink-0">
        <div className="flex items-center gap-3">
          <Link to="/meetings" className="text-fg-4 hover:text-fg transition-colors"><ArrowLeft size={16} /></Link>
          <div>
            <div className="flex items-center gap-2.5">
              {meetingStatus === 'live' && (
                <div className="flex items-center gap-1.5">
                  <div className="pulse-dot" style={{ width: 8, height: 8 }} />
                  <span className="text-[11px] font-bold text-ok uppercase tracking-[0.08em]">Live</span>
                </div>
              )}
              <h1 className="text-[14px] font-bold">{meetingTitle}</h1>
            </div>
            <div className="flex items-center gap-2 text-[11px] text-fg-4 mt-0.5">
              {meetingStatus === 'ended' && <span className="text-fg-3">Ended</span>}
              <span>{onlineCount} online</span>
              <span>{chunks.length} chunks</span>
            </div>
          </div>
        </div>

        <div className="flex items-center gap-3">
          {connected
            ? <span className="flex items-center gap-1.5 text-[10px] font-medium text-ok"><span className="w-1.5 h-1.5 rounded-full bg-ok shadow-[0_0_5px_hsla(142,70%,55%,0.5)]" />Connected</span>
            : <span className="flex items-center gap-1.5 text-[10px] font-medium text-warn"><span className="w-1.5 h-1.5 rounded-full bg-warn" />Reconnecting...</span>
          }
          <div className="flex -space-x-1.5">
            {participants.slice(0, 5).map(p => (
              <Avatar key={p.account_id} name={p.email} size="sm" className="border-2 border-surface-3" />
            ))}
          </div>
        </div>
      </div>

      {/* Task notification toast */}
      {newTaskFlash && (
        <div className="mx-6 mt-3 px-4 py-2.5 bg-accent-light border border-accent/20 rounded-lg flex items-center gap-2 animate-pulse">
          <Sparkles size={14} className="text-accent shrink-0" />
          <span className="text-[12px] font-medium">New task detected: {newTaskFlash}</span>
        </div>
      )}

      {/* Main content: transcript left, tasks right */}
      <div className="flex flex-1 min-h-0">
        {/* Transcript panel */}
        <div className="flex-1 flex flex-col min-w-0 border-r border-surface-4">
          <div className="px-5 py-2.5 border-b border-border-light flex items-center gap-2">
            <Mic size={13} className="text-fg-4" />
            <span className="text-[11px] font-bold text-fg-3 uppercase tracking-[0.12em]">Transcript</span>
          </div>
          <div className="flex-1 overflow-y-auto px-5 py-3">
            {chunks.length === 0 ? (
              <div className="text-center py-12 text-fg-4 text-[12px]">
                {meetingStatus === 'live' ? 'Waiting for transcript...' : 'No transcript available'}
              </div>
            ) : (
              chunks.map((c, i) => {
                const prevChunk = i > 0 ? chunks[i - 1] : null
                const sameSpeaker = prevChunk && prevChunk.speaker_name === c.speaker_name
                return (
                  <div key={i} className="mb-3">
                    {!sameSpeaker && (
                      <div className="flex items-center gap-2 mb-1">
                        <span className="text-[11px] font-bold" style={{ color: speakerColor(c.speaker_name || 'Unknown') }}>{c.speaker_name || 'Unknown'}</span>
                      </div>
                    )}
                    <p className="text-[12.5px] text-fg leading-[1.65] ml-0">{c.text}</p>
                  </div>
                )
              })
            )}
            <div ref={transcriptEndRef} />
          </div>

          {/* Summary strip */}
          {summary && (
            <div className="px-5 py-3 border-t border-border-light bg-surface-2">
              <div className="flex items-center gap-1.5 mb-1">
                <Sparkles size={11} className="text-accent" />
                <span className="text-[10px] font-bold text-fg-3 uppercase tracking-[0.08em]">AI Summary</span>
              </div>
              <p className="text-[11px] text-fg-2 leading-relaxed line-clamp-3">{summary}</p>
            </div>
          )}
        </div>

        {/* Tasks panel */}
        <div className="w-[340px] shrink-0 flex flex-col">
          <div className="px-4 py-2.5 border-b border-border-light flex items-center justify-between">
            <div className="flex items-center gap-2">
              <CheckSquare size={13} className="text-fg-4" />
              <span className="text-[11px] font-bold text-fg-3 uppercase tracking-[0.12em]">Action Items</span>
              {draftTasks.length > 0 && (
                <span className="text-[10px] font-semibold bg-accent-light text-accent px-1.5 py-0.5 rounded-full">{draftTasks.length}</span>
              )}
            </div>
            {draftTasks.length > 0 && (
              <button onClick={selectAll} className="text-[10px] text-accent hover:underline">Select all</button>
            )}
          </div>

          <div className="flex-1 overflow-y-auto px-4 py-2">
            {tasks.length === 0 ? (
              <div className="text-center py-12 text-fg-4 text-[12px]">
                {meetingStatus === 'live' ? 'Tasks will appear as AI detects them...' : 'No tasks detected'}
              </div>
            ) : (
              <>
                {/* Draft tasks */}
                {draftTasks.map(t => (
                  <div
                    key={t.task_id}
                    onClick={() => toggleTask(t.task_id)}
                    className={`flex items-start gap-2.5 px-3 py-2.5 rounded-[10px] mb-2 cursor-pointer transition-colors ${
                      t.selected ? 'bg-accent-light border border-accent/20 shadow-[inset_0_1px_0_hsla(0,0%,100%,0.04)]' : 'hover:bg-surface-4/40 border border-transparent'
                    }`}
                  >
                    <div className={`w-4 h-4 rounded border-[1.5px] shrink-0 mt-0.5 flex items-center justify-center transition-colors ${
                      t.selected ? 'bg-accent border-accent' : 'border-fg-5'
                    }`}>
                      {t.selected && <Check size={10} className="text-accent-fg" />}
                    </div>
                    <div className="flex-1 min-w-0">
                      <div className="text-[12px] font-medium leading-[1.45]">{t.title}</div>
                      {t.assignee_name && (
                        <div className="text-[10px] text-fg-4 mt-1 flex items-center gap-1">
                          <Avatar name={t.assignee_name} size="sm" /> {t.assignee_name}
                        </div>
                      )}
                    </div>
                    {t.pushing && <div className="spinner" style={{ width: 14, height: 14, borderWidth: 2 }} />}
                  </div>
                ))}

                {/* Synced tasks */}
                {syncedTasks.length > 0 && (
                  <>
                    <div className="text-[9px] font-medium text-fg-4 uppercase tracking-wider mt-3 mb-1.5 px-3">Pushed to Lark</div>
                    {syncedTasks.map(t => (
                      <div key={t.task_id} className="flex items-start gap-2.5 px-3 py-2 rounded-[10px] mb-1 opacity-50">
                        <div className="w-4 h-4 rounded border-[1.5px] bg-ok border-ok shrink-0 mt-0.5 flex items-center justify-center">
                          <Check size={10} className="text-white" />
                        </div>
                        <div className="flex-1 min-w-0">
                          <div className="text-[12px] font-medium leading-snug line-through">{t.title}</div>
                          {t.assignee_name && <div className="text-[10px] text-fg-4 mt-0.5">{t.assignee_name}</div>}
                        </div>
                        <span className="text-[9px] font-medium text-ok">Synced</span>
                      </div>
                    ))}
                  </>
                )}
              </>
            )}
          </div>

          {/* Push bar */}
          {draftTasks.length > 0 && (
            <div className="px-4 py-3 border-t border-border-light">
              <button
                onClick={pushSelected}
                disabled={selectedCount === 0}
                className="w-full inline-flex items-center justify-center gap-1.5 text-[12px] font-semibold h-10 rounded-lg bg-[hsl(0_0%_98%)] text-[hsl(240_8%_8%)] hover:opacity-90 disabled:opacity-30 disabled:cursor-not-allowed transition-all"
              >
                <Send size={13} />
                {selectedCount > 0 ? `Push ${selectedCount} to Lark` : 'Select tasks to push'}
              </button>
            </div>
          )}

          {/* Decisions */}
          {decisions.length > 0 && (
            <div className="px-4 py-3 border-t border-border-light">
              <div className="text-[10px] font-bold text-fg-3 uppercase tracking-[0.08em] mb-2">Decisions</div>
              {decisions.map((d, i) => (
                <div key={i} className="flex items-start gap-2 mb-1.5">
                  <span className="w-1 h-1 rounded-full bg-warn mt-1.5 shrink-0" />
                  <span className="text-[11px] text-fg-2 leading-relaxed">{d}</span>
                </div>
              ))}
            </div>
          )}
        </div>
      </div>

      {/* Bottom dock */}
      {meetingStatus === 'live' && (
        <div className="bottom-dock">
          <div className="flex items-center gap-1.5">
            <div className="pulse-dot" style={{ width: 6, height: 6 }} />
            <span className="text-[10px] font-semibold text-ok">Recording</span>
          </div>
          <div className="w-px h-[18px] bg-surface-4" />
          <span className="text-[11px] font-semibold text-fg-4 tabular-nums">
            <Clock size={11} className="inline -mt-0.5 mr-1" />Live
          </span>
          <div className="w-px h-[18px] bg-surface-4" />
          <button className="flex items-center gap-1.5 px-3.5 py-[7px] rounded-full text-[11px] font-semibold bg-surface-4 text-fg hover:bg-border transition-colors">
            <Mic size={13} /> Mute
          </button>
        </div>
      )}
    </div>
  )
}
