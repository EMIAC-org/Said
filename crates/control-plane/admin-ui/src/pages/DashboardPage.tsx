import { useState, useEffect } from 'react'
import { useNavigate } from 'react-router'
import { ArrowRight, Sparkles, Mic, TrendingUp } from 'lucide-react'
import { apiJson } from '../api'
import { useAuth } from '../hooks/useAuth'
import { StatusPill } from '../components/StatusPill'
import { Avatar } from '../components/Avatar'
import { ErrorBox, Loading } from '../components/States'
import { formatDate, duration } from '../utils'
import type { Meeting } from '../types'

export function DashboardPage() {
  const { org } = useAuth()
  const navigate = useNavigate()
  const [meetings, setMeetings] = useState<Meeting[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')

  useEffect(() => {
    apiJson<{ meetings: Meeting[] }>('/v1/meetings')
      .then(d => setMeetings(d.meetings || []))
      .catch(e => setError(e.message))
      .finally(() => setLoading(false))
  }, [])

  if (loading) return <Loading />
  if (error) return <ErrorBox title="Failed to load" message={error} />

  const total = meetings.length
  const live = meetings.filter(m => m.status === 'live').length
  const ended = meetings.filter(m => m.status === 'ended').length
  const scheduled = meetings.filter(m => m.status === 'scheduled').length
  const recent = meetings.slice(0, 5)
  const completionRate = total > 0 ? Math.round((ended / total) * 100) : 0

  const chartPoints = [0, 140, 50, 130, 100, 120, 150, 115, 200, 100, 250, 110, 300, 80, 350, 90, 400, 60, 450, 70, 500, 40, 550, 50, 600, 30]
  const polyline = []
  for (let i = 0; i < chartPoints.length; i += 2) polyline.push(`${chartPoints[i]},${chartPoints[i + 1]}`)
  const lineStr = polyline.join(' ')
  const areaStr = `0,160 ${lineStr} 600,160`

  const gaugeRadius = 70
  const gaugeCirc = Math.PI * gaugeRadius
  const gaugeOffset = gaugeCirc - (gaugeCirc * completionRate) / 100

  return (
    <>
      {/* Row 1: AI Insights | Meeting Overview | Performance */}
      <div className="grid grid-cols-[1fr_2fr_1fr] gap-4 mb-4">

        {/* AI Insights */}
        <div className="card-gradient flex flex-col justify-between min-h-[220px]">
          <div className="text-[10px] font-semibold text-fg-4 uppercase tracking-wider mb-3">AI Insights</div>
          <div className="flex-1 flex flex-col justify-center">
            <div className="flex items-start gap-2.5 mb-3">
              <div className="w-8 h-8 rounded-lg bg-accent/10 flex items-center justify-center shrink-0 mt-0.5">
                <Sparkles size={14} className="text-accent" />
              </div>
              <p className="text-[13px] text-fg-2 leading-relaxed">
                {total > 0
                  ? `You've completed ${ended} meetings this month with a ${completionRate}% completion rate.`
                  : 'Create your first meeting to see AI-powered insights here.'}
              </p>
            </div>
          </div>
          <div className="flex gap-1 mt-2">
            {[0, 1, 2, 3].map(i => (
              <div key={i} className={`w-1.5 h-1.5 rounded-full ${i === 0 ? 'bg-accent' : 'bg-fg-5'}`} />
            ))}
          </div>
        </div>

        {/* Meeting Overview — area chart */}
        <div className="card">
          <div className="flex items-center justify-between mb-1">
            <span className="text-[10px] font-semibold text-fg-4 uppercase tracking-wider">Meeting Overview</span>
            <div className="flex items-center gap-2">
              <span className="text-[10px] font-semibold px-2.5 py-1 rounded-full bg-accent-light text-fg-3 uppercase tracking-wide">All meetings</span>
              <span className="text-[10px] font-semibold px-2.5 py-1 rounded-full bg-surface-4 text-fg-4">{ended} completed</span>
            </div>
          </div>
          <div className="flex items-baseline gap-2 mb-1">
            <span className="text-[32px] font-semibold tracking-tighter leading-none">{total}</span>
            {total > 0 && (
              <span className="text-[10px] font-semibold text-ok bg-ok-bg px-1.5 py-0.5 rounded">
                <TrendingUp size={10} className="inline -mt-0.5 mr-0.5" />{completionRate}%
              </span>
            )}
          </div>
          <div className="text-[11px] text-fg-4 mb-3">Total meetings &middot; {live} live now</div>

          <div className="relative">
            <svg viewBox="0 0 600 160" preserveAspectRatio="none" className="w-full block" style={{ height: 130 }}>
              <defs>
                <linearGradient id="areaGrad" x1="0" y1="0" x2="0" y2="1">
                  <stop offset="0%" stopColor="var(--color-accent)" stopOpacity="0.2" />
                  <stop offset="100%" stopColor="var(--color-accent)" stopOpacity="0" />
                </linearGradient>
                <linearGradient id="lineGrad" x1="0" y1="0" x2="1" y2="0">
                  <stop offset="0%" stopColor="var(--color-accent)" stopOpacity="0.3" />
                  <stop offset="40%" stopColor="var(--color-accent)" />
                  <stop offset="100%" stopColor="var(--color-accent)" />
                </linearGradient>
              </defs>
              {[0, 40, 80, 120, 160].map(y => (
                <line key={y} x1="0" y1={y} x2="600" y2={y} stroke="var(--color-border-light)" strokeWidth="0.5" />
              ))}
              <polygon points={areaStr} fill="url(#areaGrad)" />
              <polyline points={lineStr} fill="none" stroke="url(#lineGrad)" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" />
              <circle cx="500" cy="40" r="4" fill="var(--color-surface-3)" stroke="var(--color-accent)" strokeWidth="2" />
              <circle cx="500" cy="40" r="8" fill="none" stroke="var(--color-accent)" strokeWidth="1" opacity="0.2" />
            </svg>
          </div>
        </div>

        {/* AI Performance — gauge */}
        <div className="card flex flex-col">
          <div className="flex items-center justify-between mb-1">
            <span className="text-[10px] font-semibold text-fg-4 uppercase tracking-wider">Performance</span>
            <button className="flex items-center gap-1 text-[10px] text-fg-4 hover:text-fg-3"><ArrowRight size={10} /></button>
          </div>
          <div className="flex items-baseline gap-2 mb-1">
            <span className="text-[32px] font-semibold tracking-tighter leading-none">{ended}</span>
          </div>
          <div className="text-[11px] text-fg-4 mb-1">Completed meetings</div>

          <div className="flex flex-col items-center flex-1 justify-center py-2">
            <svg width={160} height={90} viewBox="0 0 200 105">
              <path d="M 20 95 A 80 80 0 0 1 180 95" fill="none" stroke="var(--color-border)" strokeWidth="12" strokeLinecap="round" />
              <path d="M 20 95 A 80 80 0 0 1 180 95" fill="none" stroke="var(--color-accent)" strokeWidth="12" strokeLinecap="round"
                strokeDasharray={gaugeCirc} strokeDashoffset={gaugeOffset}
                style={{ filter: 'drop-shadow(0 0 6px var(--color-accent-glow))' }}
              />
            </svg>
            <div className="text-center -mt-2">
              <div className="text-xs text-fg-4">Percentage</div>
              <div className="text-2xl font-semibold tracking-tight">{completionRate}%</div>
            </div>
          </div>

          <div className="flex items-center justify-center gap-4 text-[10px] text-fg-4 mt-auto pt-2 border-t border-border-light">
            <span className="flex items-center gap-1"><span className="w-2 h-2 rounded-full bg-accent" />Completed</span>
            <span className="flex items-center gap-1"><span className="w-2 h-2 rounded-full bg-border" />Remaining</span>
          </div>
        </div>
      </div>

      {/* Row 2: Recent Meetings | Stats */}
      <div className="grid grid-cols-[3fr_2fr] gap-4">

        {/* Recent Meetings */}
        <div className="card !p-0">
          <div className="flex items-center justify-between px-5 py-4">
            <span className="text-[13px] font-semibold">Recent Meetings</span>
            <button onClick={() => navigate('/meetings')} className="flex items-center gap-1 text-[11px] text-fg-4 hover:text-fg-3 transition-colors">
              View all <ArrowRight size={12} />
            </button>
          </div>
          {recent.length === 0 ? (
            <div className="text-center py-10 text-fg-3 border-t border-border-light">
              <h3 className="text-[14px] font-semibold text-fg mb-1">No meetings yet</h3>
              <p className="text-[12px]">Create your first meeting to get started.</p>
            </div>
          ) : (
            <table className="w-full">
              <thead>
                <tr className="border-t border-border-light">
                  {['Meeting', 'Date', 'Duration', 'Status'].map(h => (
                    <th key={h} className="text-[10px] font-medium text-fg-4 text-left px-5 py-2.5 uppercase tracking-wider">{h}</th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {recent.map(m => (
                  <tr key={m.id} className="border-t border-border-light cursor-pointer hover:bg-surface-4/30 transition-colors" onClick={() => navigate(`/meetings/${m.id}`)}>
                    <td className="px-5 py-3">
                      <div className="flex items-center gap-2.5">
                        <Avatar name={m.title} size="sm" />
                        <span className="text-[13px] font-medium">{m.title}</span>
                      </div>
                    </td>
                    <td className="text-[12px] text-fg-3 px-5 py-3">{formatDate(m.created_at)}</td>
                    <td className="text-[12px] text-fg-3 px-5 py-3">{duration(m.started_at, m.ended_at)}</td>
                    <td className="px-5 py-3"><StatusPill status={m.status} /></td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>

        {/* Stats — breakdown */}
        <div className="card">
          <div className="flex items-center justify-between mb-4">
            <span className="text-[13px] font-semibold">Meeting Stats</span>
            <button className="flex items-center gap-1 text-[11px] text-fg-4 hover:text-fg-3">Details <ArrowRight size={12} /></button>
          </div>

          <div className="flex items-baseline gap-2 mb-4">
            <span className="text-[28px] font-semibold tracking-tighter">{total}</span>
            <span className="text-[11px] text-fg-4">total this month</span>
          </div>

          <div className="space-y-3">
            <StatBar label="Completed" value={ended} total={total} color="bg-ok" />
            <StatBar label="Live" value={live} total={total} color="bg-live" />
            <StatBar label="Scheduled" value={scheduled} total={total} color="bg-accent" />
          </div>

          <div className="mt-6 pt-4 border-t border-border-light">
            <div className="flex items-center gap-2 mb-2">
              <Mic size={13} className="text-fg-4" />
              <span className="text-[11px] font-medium text-fg-3">Transcription</span>
            </div>
            <div className="text-[22px] font-semibold tracking-tighter mb-1">--</div>
            <div className="text-[11px] text-fg-4">Words transcribed</div>
            <svg viewBox="0 0 200 40" preserveAspectRatio="none" className="w-full mt-3" style={{ height: 36 }}>
              <polyline points="0,30 25,25 50,20 75,24 100,16 125,12 150,18 175,8 200,5" fill="none" stroke="var(--color-accent)" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
              <circle cx="200" cy="5" r="2.5" fill="var(--color-accent)" />
            </svg>
          </div>
        </div>
      </div>
    </>
  )
}

function StatBar({ label, value, total, color }: { label: string; value: number; total: number; color: string }) {
  const pct = total > 0 ? (value / total) * 100 : 0
  return (
    <div>
      <div className="flex items-center justify-between mb-1.5">
        <span className="text-[12px] text-fg-3">{label}</span>
        <span className="text-[12px] font-medium">{value}</span>
      </div>
      <div className="h-2 bg-border-light rounded-full overflow-hidden">
        <div className={`h-full rounded-full ${color} transition-all`} style={{ width: `${Math.max(pct, 2)}%` }} />
      </div>
    </div>
  )
}
