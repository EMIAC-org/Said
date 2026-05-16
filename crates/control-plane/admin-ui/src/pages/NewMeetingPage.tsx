import { useState, useEffect, useMemo } from 'react'
import { useNavigate, Link } from 'react-router'
import { ArrowLeft, Search, X, Plus, Check } from 'lucide-react'
import { apiJson } from '../api'
import { useAuth } from '../hooks/useAuth'
import { Avatar } from '../components/Avatar'
import type { OrgMember } from '../types'

export function NewMeetingPage() {
  const navigate = useNavigate()
  const { org } = useAuth()
  const [members, setMembers] = useState<OrgMember[]>([])
  const [title, setTitle] = useState('')
  const [agenda, setAgenda] = useState('')
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const [search, setSearch] = useState('')
  const [error, setError] = useState('')
  const [loading, setLoading] = useState(false)

  useEffect(() => {
    if (org?.org?.id)
      apiJson<{ members: OrgMember[] }>(`/v1/orgs/${org.org.id}/members`).then(d => setMembers(d.members || [])).catch(() => {})
  }, [org])

  const toggle = (id: string) => setSelected(p => { const n = new Set(p); n.has(id) ? n.delete(id) : n.add(id); return n })
  const remove = (id: string) => setSelected(p => { const n = new Set(p); n.delete(id); return n })

  const selectedMembers = members.filter(m => selected.has(m.account_id))

  const departments = useMemo(() => {
    const map: Record<string, OrgMember[]> = {}
    const filtered = search
      ? members.filter(m => (m.lark_name || m.account_id).toLowerCase().includes(search.toLowerCase()))
      : members
    for (const m of filtered) {
      const dept = m.lark_department || 'Team'
      if (!map[dept]) map[dept] = []
      map[dept].push(m)
    }
    return map
  }, [members, search])

  const submit = async (startNow: boolean) => {
    if (!title.trim()) { setError('Title is required.'); return }
    setLoading(true); setError('')
    try {
      const d = await apiJson<{ meeting: { id: string } }>('/v1/meetings', {
        method: 'POST',
        body: JSON.stringify({ title: title.trim(), agenda: agenda.trim() || null, participant_ids: [...selected] }),
      })
      if (startNow) {
        await apiJson(`/v1/meetings/${d.meeting.id}/start`, { method: 'POST' }).catch(() => {})
      }
      navigate(`/meetings/${d.meeting.id}`)
    } catch (e) { setError((e as Error).message); setLoading(false) }
  }

  return (
    <>
      <Link to="/" className="inline-flex items-center gap-1.5 text-xs text-fg-4 hover:text-fg mb-5 transition-colors">
        <ArrowLeft size={14} /> Back to Dashboard
      </Link>

      <h1 className="text-xl font-semibold tracking-tight mb-1">Create a meeting</h1>
      <p className="text-[12px] text-fg-4 mb-6">Participants will see it in their Said app and can join when ready.</p>

      <div className="grid grid-cols-[2fr_1fr] gap-4">
        {/* Left — form */}
        <div className="space-y-4">
          {/* Title */}
          <div className="card">
            <label className="block text-[10px] font-medium text-fg-4 uppercase tracking-wider mb-2">Meeting Title</label>
            <input
              type="text"
              className="w-full px-4 py-2.5 text-[13px] bg-floor border border-border rounded-xl outline-none focus:border-accent focus:ring-2 focus:ring-accent-light transition placeholder:text-fg-5"
              placeholder="Sprint Planning — Week 22"
              value={title}
              onChange={e => setTitle(e.target.value)}
            />
          </div>

          {/* Agenda */}
          <div className="card">
            <label className="block text-[10px] font-medium text-fg-4 uppercase tracking-wider mb-2">Agenda (Optional)</label>
            <textarea
              className="w-full px-4 py-2.5 text-[13px] bg-floor border border-border rounded-xl outline-none focus:border-accent focus:ring-2 focus:ring-accent-light transition resize-y placeholder:text-fg-5"
              rows={3}
              placeholder={"1. Review last week's velocity\n2. Assign stories for this sprint\n3. Blockers check"}
              value={agenda}
              onChange={e => setAgenda(e.target.value)}
            />
          </div>

          {/* Participants */}
          <div className="card !p-0 overflow-hidden">
            <div className="px-5 py-3.5">
              <label className="block text-[10px] font-medium text-fg-4 uppercase tracking-wider mb-2">Participants</label>

              {selectedMembers.length > 0 && (
                <div className="flex flex-wrap gap-1.5 mb-3">
                  {selectedMembers.map(m => {
                    const name = m.lark_name || m.account_id
                    return (
                      <div key={m.account_id} className="inline-flex items-center gap-1.5 bg-accent-light rounded-lg pl-1 pr-2 py-1">
                        <Avatar name={name} size="sm" />
                        <span className="text-[11px] font-medium">{name}</span>
                        <button onClick={() => remove(m.account_id)} className="text-fg-4 hover:text-fg transition-colors ml-0.5">
                          <X size={11} />
                        </button>
                      </div>
                    )
                  })}
                </div>
              )}

              <div className="flex items-center gap-2 px-3 py-2 bg-floor rounded-xl border border-border">
                <Search size={13} className="text-fg-4 shrink-0" />
                <input
                  type="text"
                  className="flex-1 text-[13px] bg-transparent outline-none placeholder:text-fg-5"
                  placeholder="Search Lark contacts..."
                  value={search}
                  onChange={e => setSearch(e.target.value)}
                />
              </div>
              <p className="text-[10px] text-fg-5 mt-1.5">Showing contacts from your Lark workspace</p>
            </div>

            {members.length > 0 ? (
              <div className="border-t border-border-light">
                {Object.entries(departments).map(([dept, deptMembers]) => (
                  <div key={dept}>
                    <div className="px-5 py-2 bg-floor text-[9px] font-medium text-fg-4 uppercase tracking-wider border-b border-border-light">
                      Lark Contacts — {dept}
                    </div>
                    {deptMembers.map((m, i) => {
                      const name = m.lark_name || m.account_id
                      const isSelected = selected.has(m.account_id)
                      return (
                        <div
                          key={m.account_id}
                          className={`flex items-center gap-3 px-5 py-2.5 cursor-pointer hover:bg-accent-light/50 transition-colors ${i < deptMembers.length - 1 ? 'border-b border-border-light' : ''}`}
                          onClick={() => toggle(m.account_id)}
                        >
                          <Avatar name={name} size="sm" />
                          <span className="text-[13px] font-medium flex-1">{name}</span>
                          {isSelected ? (
                            <span className="text-[11px] text-ok font-medium flex items-center gap-1"><Check size={12} /> Added</span>
                          ) : (
                            <span className="text-[11px] text-fg-4 font-medium flex items-center gap-1"><Plus size={12} /> Add</span>
                          )}
                        </div>
                      )
                    })}
                  </div>
                ))}
              </div>
            ) : (
              <div className="border-t border-border-light px-5 py-8 text-center text-[12px] text-fg-4">
                No team members found. Sync your org via Lark first.
              </div>
            )}
          </div>
        </div>

        {/* Right — actions + settings */}
        <div className="space-y-4">
          <div className="card">
            <label className="block text-[10px] font-medium text-fg-4 uppercase tracking-wider mb-3">When</label>
            <div className="space-y-2">
              <button
                onClick={() => submit(true)}
                disabled={loading}
                className="w-full inline-flex items-center justify-center gap-2 text-[13px] font-semibold py-2.5 rounded-xl bg-accent text-accent-fg hover:bg-accent-hover shadow-[0_2px_8px_var(--color-accent-glow)] disabled:opacity-40 transition-all"
              >
                {loading && <div className="spinner" style={{ width: 14, height: 14, borderWidth: 2 }} />}
                Start Now
              </button>
              <button
                onClick={() => submit(false)}
                disabled={loading}
                className="w-full inline-flex items-center justify-center gap-2 text-[13px] font-medium py-2.5 rounded-xl border border-border text-fg-2 hover:bg-floor hover:border-fg-5 disabled:opacity-40 transition-colors"
              >
                Schedule for Later
              </button>
            </div>
          </div>

          <div className="card">
            <label className="block text-[10px] font-medium text-fg-4 uppercase tracking-wider mb-3">AI Settings</label>
            <div className="flex flex-col gap-3">
              <label className="flex items-center gap-2.5 cursor-pointer">
                <input type="checkbox" defaultChecked className="w-4 h-4 accent-[var(--color-accent)] rounded" />
                <span className="text-[12px]">Live summary (every 30s)</span>
              </label>
              <label className="flex items-center gap-2.5 cursor-pointer">
                <input type="checkbox" defaultChecked className="w-4 h-4 accent-[var(--color-accent)] rounded" />
                <span className="text-[12px]">Extract action items</span>
              </label>
            </div>
          </div>

          {error && <div className="bg-live-bg border border-live/20 rounded-2xl px-4 py-3 text-[12px] text-live">{error}</div>}
        </div>
      </div>
    </>
  )
}
