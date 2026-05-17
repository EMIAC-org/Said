import { useState, useEffect } from 'react'
import { apiJson } from '../api'
import { useAuth } from '../hooks/useAuth'
import { Avatar } from '../components/Avatar'
import { RolePill } from '../components/StatusPill'
import { Loading, Empty, ErrorBox } from '../components/States'
import { formatDate } from '../utils'
import type { OrgMember } from '../types'

export function TeamPage() {
  const { org } = useAuth()
  const [members, setMembers] = useState<OrgMember[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')

  useEffect(() => {
    if (!org?.org?.id) { setLoading(false); return }
    apiJson<{ members: OrgMember[] }>(`/v1/orgs/${org.org.id}/members`)
      .then(d => setMembers(d.members || []))
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
                {['Name', 'Department', 'Role', 'Joined'].map(h => (
                  <th key={h} className="text-[10px] font-medium text-fg-4 text-left px-5 py-3 border-b border-border uppercase tracking-wider">{h}</th>
                ))}
              </tr>
            </thead>
            <tbody>
              {members.map(m => {
                const name = m.lark_name || `User ${(m.account_id || '?').substring(0, 8)}`
                return (
                  <tr key={m.account_id} className="hover:bg-surface-4/30 transition-colors">
                    <td className="px-5 py-3.5 border-b border-border-light">
                      <div className="flex items-center gap-2.5">
                        <Avatar name={name} size="sm" />
                        <span className="text-[13px] font-medium">{name}</span>
                      </div>
                    </td>
                    <td className="text-[12px] text-fg-3 px-5 py-3.5 border-b border-border-light">{m.lark_department || '--'}</td>
                    <td className="px-5 py-3.5 border-b border-border-light"><RolePill role={m.role} /></td>
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
