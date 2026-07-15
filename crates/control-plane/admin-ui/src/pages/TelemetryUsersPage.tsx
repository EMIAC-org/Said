import { useEffect, useState } from 'react'
import { useNavigate, useSearchParams } from 'react-router'
import { apiJson } from '../api'
import { useAuth } from '../hooks/useAuth'
import { Avatar } from '../components/Avatar'
import { TelemetryTabs } from '../components/telemetry/TelemetryTabs'
import { pct, relTime, speechLabel, usd } from '../components/telemetry/format'
import { Loading, Empty, ErrorBox } from '../components/States'
import type { TelemetryUserRow } from '../types'

export function TelemetryUsersPage() {
  const { org } = useAuth()
  const navigate = useNavigate()
  const [searchParams, setSearchParams] = useSearchParams()
  const days = Number(searchParams.get('days') || '30')
  const [q, setQ] = useState(searchParams.get('q') || '')
  const [users, setUsers] = useState<TelemetryUserRow[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')

  useEffect(() => {
    const orgId = org?.org?.id
    if (!orgId) {
      setLoading(false)
      return
    }
    const params = new URLSearchParams({ days: String(days) })
    const trimmed = q.trim()
    if (trimmed) params.set('q', trimmed)
    setLoading(true)
    apiJson<{ users: TelemetryUserRow[] }>(`/v1/orgs/${orgId}/telemetry/users?${params}`)
      .then(d => setUsers(d.users || []))
      .catch(e => setError(e.message))
      .finally(() => setLoading(false))
  }, [org, days, q])

  if (!org?.org?.id) {
    return (
      <div className="card p-6 text-[13px] text-fg-3">
        Select an organization workspace to view desktop telemetry.
      </div>
    )
  }

  return (
    <>
      <div className="mb-4">
        <h1 className="text-[15px] font-semibold text-fg">Desktop Telemetry</h1>
        <p className="text-[12px] text-fg-4 mt-1">
          {org.org.name} · all org members · click a row for full profile
        </p>
      </div>

      <TelemetryTabs />

      <div className="flex items-center justify-between gap-3 mb-4 flex-wrap">
        <input
          type="search"
          value={q}
          onChange={e => setQ(e.target.value)}
          placeholder="Search by name or email…"
          className="text-[12px] px-3 py-2 rounded-lg border border-border bg-surface-2 text-fg max-w-[280px] w-full focus:outline-none focus:border-accent"
        />
        <select
          value={days}
          onChange={e => setSearchParams({ days: e.target.value, ...(q.trim() ? { q: q.trim() } : {}) })}
          className="text-[12px] px-2.5 py-1.5 rounded-lg border border-border bg-surface-2 text-fg"
        >
          <option value="7">Last 7 days</option>
          <option value="30">Last 30 days</option>
          <option value="90">Last 90 days</option>
        </select>
      </div>

      {loading ? (
        <Loading />
      ) : error ? (
        <ErrorBox title="Failed to load users" message={error} />
      ) : !users.length ? (
        <Empty title="No members" message="No team members match your search." />
      ) : (
        <div className="card !p-0 overflow-hidden">
          <div className="overflow-x-auto overscroll-x-contain">
          <table className="w-full min-w-[820px] table-fixed">
            <thead>
              <tr>
                {[
                  ['User', 'w-[27%]'],
                  ['Usage', 'w-[14%]'],
                  ['Quality', 'w-[16%]'],
                  ['STT', 'w-[18%]'],
                  ['Cost', 'w-[13%]'],
                  ['Activity', 'w-[12%]'],
                ].map(([h, width]) => (
                  <th
                    key={h}
                    className={`${width} text-[10px] font-medium text-fg-4 text-left px-5 py-3 border-b border-border uppercase tracking-wider`}
                  >
                    {h}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {users.map(u => {
                const name = u.lark_name || u.email || `User ${u.account_id.substring(0, 8)}`
                return (
                  <tr
                    key={u.account_id}
                    className="cursor-pointer hover:bg-surface-4/30 transition-colors"
                    onClick={() =>
                      navigate(`/telemetry/users/${u.account_id}?days=${days}`)
                    }
                  >
                    <td className="px-5 py-3.5 border-b border-border-light">
                      <div className="flex items-center gap-2.5">
                        <Avatar name={name} size="sm" />
                        <div>
                          <div className="text-[13px] font-medium">{name}</div>
                          <div className="text-[10px] text-fg-4">{u.email}</div>
                        </div>
                      </div>
                    </td>
                    <td className="px-5 py-3.5 border-b border-border-light">
                      <div className="text-[12px] tabular-nums">{u.runs.toLocaleString()} runs</div>
                      <div className="text-[10px] text-fg-4 tabular-nums mt-0.5">{u.audio_minutes} min</div>
                    </td>
                    <td className="px-5 py-3.5 border-b border-border-light">
                      <div className="text-[12px] tabular-nums">{pct(u.acceptance_rate)} accepted</div>
                      <div className="text-[10px] text-fg-4 tabular-nums mt-0.5">{pct(u.heavy_edit_rate)} heavy edit</div>
                    </td>
                    <td
                      title={u.primary_speech || undefined}
                      className="text-[11px] px-5 py-3.5 border-b border-border-light text-fg-3"
                    >
                      {speechLabel(u.primary_speech)}
                    </td>
                    <td className="text-[12px] tabular-nums px-5 py-3.5 border-b border-border-light">
                      <div>{usd(u.costs.total_usd)}</div>
                      <div className="text-[10px] text-fg-4">
                        {u.costs.coverage_rate >= 1 ? 'Fully tracked' : 'Partial estimate'}
                      </div>
                    </td>
                    <td className="px-5 py-3.5 border-b border-border-light">
                      <div className="text-[11px] text-fg-3">{relTime(u.last_active_at)}</div>
                      <div className={`text-[10px] mt-0.5 ${u.desktop_active ? 'text-ok' : 'text-fg-4'}`}>
                        {u.desktop_active ? 'Online' : 'Offline'}
                      </div>
                    </td>
                  </tr>
                )
              })}
            </tbody>
          </table>
          </div>
        </div>
      )}
    </>
  )
}
