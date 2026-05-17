import { useState, useEffect } from 'react'
import { useNavigate } from 'react-router'
import { apiJson } from '../api'
import { StatusPill } from '../components/StatusPill'
import { Avatar } from '../components/Avatar'
import { ErrorBox, Loading, Empty } from '../components/States'
import { formatDate, duration, timeAgo, formatIstDateTime, formatMinutes } from '../utils'
import type { Meeting } from '../types'

const filters = [
  { label: 'All', value: '' },
  { label: 'Scheduled', value: 'scheduled' },
  { label: 'Live', value: 'live' },
  { label: 'Ended', value: 'ended' },
]

export function MeetingsPage() {
  const navigate = useNavigate()
  const [meetings, setMeetings] = useState<Meeting[]>([])
  const [filter, setFilter] = useState('')
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')

  useEffect(() => {
    setLoading(true)
    apiJson<{ meetings: Meeting[] }>(`/v1/meetings${filter ? `?status=${filter}` : ''}`)
      .then(d => setMeetings(d.meetings || []))
      .catch(e => setError(e.message))
      .finally(() => setLoading(false))
  }, [filter])

  if (loading) return <Loading />
  if (error) return <ErrorBox title="Failed to load" message={error} />

  return (
    <>
      <div className="flex justify-between items-center mb-5">
        <div>
          <h1 className="text-xl font-semibold tracking-tight">Meetings</h1>
          <p className="text-[12px] text-fg-4 mt-0.5">{meetings.length} total</p>
        </div>
        <div className="flex gap-1.5">
          {filters.map(f => (
            <button key={f.value} onClick={() => setFilter(f.value)}
              className={`text-[10px] font-semibold px-3.5 py-1.5 rounded-full transition-all uppercase tracking-wide ${
                filter === f.value
                  ? 'bg-[hsl(0_0%_98%)] text-[hsl(240_8%_8%)]'
                  : 'bg-surface-4 text-fg-3 hover:text-fg-2'
              }`}>
              {f.label}
            </button>
          ))}
        </div>
      </div>

      {meetings.length === 0 ? <Empty title="No meetings found" /> : (
        <div className="card !p-0 overflow-hidden">
          <table className="w-full">
            <thead>
              <tr>
                {['Title', 'Date', 'Duration', 'Status', 'Created'].map(h => (
                  <th key={h} className="text-[10px] font-medium text-fg-4 text-left px-5 py-3 border-b border-border uppercase tracking-wider">{h}</th>
                ))}
              </tr>
            </thead>
            <tbody>
              {meetings.map(m => (
                <tr key={m.id} className="cursor-pointer hover:bg-surface-4/30 transition-colors" onClick={() => navigate(`/meetings/${m.id}`)}>
                  <td className="px-5 py-3.5 border-b border-border-light">
                    <div className="flex items-center gap-2.5">
                      <Avatar name={m.title} size="sm" />
                      <span className="text-[13px] font-medium">{m.title}</span>
                    </div>
                  </td>
                  <td className="text-[12px] text-fg-3 px-5 py-3.5 border-b border-border-light">{m.scheduled_at ? formatIstDateTime(m.scheduled_at) : formatDate(m.created_at)}</td>
                  <td className="text-[12px] text-fg-3 px-5 py-3.5 border-b border-border-light">{m.scheduled_at ? formatMinutes(m.duration_minutes) : duration(m.started_at, m.ended_at)}</td>
                  <td className="px-5 py-3.5 border-b border-border-light"><StatusPill status={m.status} /></td>
                  <td className="text-[12px] text-fg-4 px-5 py-3.5 border-b border-border-light">{timeAgo(m.created_at)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </>
  )
}
