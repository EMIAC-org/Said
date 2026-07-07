import { useEffect, useMemo, useState } from 'react'
import { useSearchParams } from 'react-router'
import { AudioLines } from 'lucide-react'
import { apiJson } from '../api'
import { useAuth } from '../hooks/useAuth'
import { DictationInspector, type ResolveAccount } from '../components/telemetry/DictationInspector'
import type { TelemetryUserRow } from '../types'

/**
 * Org-wide, first-class monitoring of the raw STT → polish → kept pipeline for
 * every member. Beta/internal: intentionally not gated beyond org-admin.
 */
export function DictationsPage() {
  const { org } = useAuth()
  const [searchParams, setSearchParams] = useSearchParams()
  const days = Number(searchParams.get('days') || '30')
  const orgId = org?.org?.id

  const [users, setUsers] = useState<TelemetryUserRow[]>([])

  useEffect(() => {
    if (!orgId) return
    apiJson<{ users: TelemetryUserRow[] }>(`/v1/orgs/${orgId}/telemetry/users?days=${days}`)
      .then(d => setUsers(d.users || []))
      .catch(() => setUsers([]))
  }, [orgId, days])

  const resolveAccount: ResolveAccount = useMemo(() => {
    const byId = new Map(users.map(u => [u.account_id, u]))
    return (accountId: string) => {
      const u = byId.get(accountId)
      if (!u) return undefined
      const name = u.lark_name || u.email.split('@')[0] || accountId.slice(0, 8)
      return { name, sub: u.email }
    }
  }, [users])

  if (!orgId) {
    return (
      <div className="card p-6 text-[13px] text-fg-3">
        Select an organization workspace to monitor dictations.
      </div>
    )
  }

  return (
    <>
      <div className="mb-4 flex items-end justify-between gap-4 flex-wrap">
        <div>
          <h1 className="text-xl font-semibold tracking-tight flex items-center gap-2">
            <AudioLines size={18} className="text-accent" />
            Dictations
          </h1>
          <p className="text-[12px] text-fg-4 mt-1">
            Live STT → polish → kept pipeline for every member. Click a run to inspect transcript stages,
            edits, and learned aliases.
          </p>
        </div>
        <select
          value={days}
          onChange={e => setSearchParams({ days: e.target.value })}
          className="text-[12px] px-2.5 py-1.5 rounded-lg border border-border bg-surface-2 text-fg"
        >
          <option value="7">Last 7 days</option>
          <option value="30">Last 30 days</option>
          <option value="90">Last 90 days</option>
        </select>
      </div>

      <DictationInspector orgId={orgId} days={days} resolveAccount={resolveAccount} />
    </>
  )
}
