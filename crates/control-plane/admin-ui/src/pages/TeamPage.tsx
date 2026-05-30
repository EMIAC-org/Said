import { useState, useEffect } from 'react'
import { apiJson } from '../api'
import { useAuth } from '../hooks/useAuth'
import { Avatar } from '../components/Avatar'
import { RolePill } from '../components/StatusPill'
import { Loading, Empty, ErrorBox } from '../components/States'
import { formatDate } from '../utils'
import type { OrgMember, DesktopClient } from '../types'

function isActive(lastSeen: string): boolean {
  return Date.now() - new Date(lastSeen).getTime() < 15 * 60 * 1000
}

function AuthTag({ member }: { member: OrgMember }) {
  const lark = member.lark_connected || member.auth_source === 'lark'
  return (
    <span
      className="text-[10px] font-semibold px-2 py-0.5 rounded-full"
      style={{
        background: lark ? 'hsl(145 60% 16%)' : 'hsl(210 60% 16%)',
        color: lark ? 'hsl(145 70% 65%)' : 'hsl(210 70% 68%)',
      }}
    >
      {lark ? 'Lark' : 'Email only'}
    </span>
  )
}

export function TeamPage() {
  const { org } = useAuth()
  const [members, setMembers] = useState<OrgMember[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')

  useEffect(() => {
    if (!org?.org?.id) { setLoading(false); return }
    Promise.all([
      apiJson<{ members: OrgMember[] }>(`/v1/orgs/${org.org.id}/members`),
      apiJson<{ clients: DesktopClient[] }>(`/v1/orgs/${org.org.id}/clients`),
    ])
      .then(([membersRes, clientsRes]) => {
        const clientByAccount = new Map<string, DesktopClient>()
        for (const c of clientsRes.clients || []) {
          const existing = clientByAccount.get(c.account_id)
          if (!existing || new Date(c.last_seen_at) > new Date(existing.last_seen_at)) {
            clientByAccount.set(c.account_id, c)
          }
        }
        setMembers(
          (membersRes.members || []).map(m => ({
            ...m,
            desktop_active: clientByAccount.has(m.account_id)
              && isActive(clientByAccount.get(m.account_id)!.last_seen_at),
          })),
        )
      })
      .catch(e => setError(e.message))
      .finally(() => setLoading(false))
  }, [org])

  if (loading) return <Loading />
  if (error) return <ErrorBox title="Failed to load" message={error} />

  const orgName = org?.org?.name || ''

  return (
    <>
      <div className="mb-5">
        <h1 className="text-xl font-semibold tracking-tight">Team</h1>
        <p className="text-[12px] text-fg-4 mt-0.5">{orgName}{orgName ? ' · ' : ''}{members.length} member{members.length !== 1 ? 's' : ''}</p>
      </div>

      {!members.length ? <Empty title="No members" message="Invite team members to get started." /> : (
        <div className="card !p-0 overflow-hidden">
          <table className="w-full">
            <thead>
              <tr>
                {['Name', 'Auth', 'Department', 'Role', 'Desktop', 'Joined'].map(h => (
                  <th key={h} className="text-[10px] font-medium text-fg-4 text-left px-5 py-3 border-b border-border uppercase tracking-wider">{h}</th>
                ))}
              </tr>
            </thead>
            <tbody>
              {members.map(m => {
                const name = m.lark_name || m.email || `User ${(m.account_id || '?').substring(0, 8)}`
                return (
                  <tr key={m.account_id} className="hover:bg-surface-4/30 transition-colors">
                    <td className="px-5 py-3.5 border-b border-border-light">
                      <div className="flex items-center gap-2.5">
                        <Avatar name={name} size="sm" />
                        <div>
                          <div className="text-[13px] font-medium">{name}</div>
                          {m.email && m.lark_name && (
                            <div className="text-[10px] text-fg-4">{m.email}</div>
                          )}
                        </div>
                      </div>
                    </td>
                    <td className="px-5 py-3.5 border-b border-border-light"><AuthTag member={m} /></td>
                    <td className="text-[12px] text-fg-3 px-5 py-3.5 border-b border-border-light">{m.lark_department || '--'}</td>
                    <td className="px-5 py-3.5 border-b border-border-light"><RolePill role={m.role} /></td>
                    <td className="px-5 py-3.5 border-b border-border-light">
                      <span
                        className="inline-block w-2 h-2 rounded-full"
                        style={{
                          background: m.desktop_active ? 'hsl(145 70% 50%)' : 'hsl(var(--border))',
                          boxShadow: m.desktop_active ? '0 0 6px hsl(145 70% 50% / 0.5)' : undefined,
                        }}
                        title={m.desktop_active ? 'Desktop active' : 'Desktop offline'}
                      />
                    </td>
                    <td className="text-[12px] text-fg-3 px-5 py-3.5 border-b border-border-light">{formatDate(m.joined_at)}</td>
                  </tr>
                )
              })}
            </tbody>
          </table>
        </div>
      )}
    </>
  )
}
