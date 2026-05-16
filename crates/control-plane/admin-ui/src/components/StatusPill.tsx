const statusStyles: Record<string, string> = {
  scheduled: 'text-info bg-info-bg',
  live: 'text-live bg-live-bg',
  ended: 'text-ok bg-ok-bg',
}

const dotStyles: Record<string, string> = {
  scheduled: 'bg-info',
  live: 'bg-live animate-pulse',
  ended: 'bg-ok',
}

export function StatusPill({ status }: { status: string }) {
  const cls = statusStyles[status] || 'text-fg-3 bg-accent-light'
  const dot = dotStyles[status] || 'bg-fg-4'
  const label = status ? status.charAt(0).toUpperCase() + status.slice(1) : '--'
  return (
    <span className={`inline-flex items-center gap-1.5 text-[10px] font-semibold px-2.5 py-1 rounded-full ${cls}`}>
      <span className={`w-[5px] h-[5px] rounded-full ${dot}`} />
      {label}
    </span>
  )
}

const roleStyles: Record<string, string> = {
  admin: 'text-info bg-info-bg',
  company_admin: 'text-info bg-info-bg',
  manager: 'text-warn bg-warn-bg',
}

export function RolePill({ role }: { role: string }) {
  if (!role) return null
  const r = role.toLowerCase()
  const cls = roleStyles[r] || 'text-fg-3 bg-accent-light'
  const labels: Record<string, string> = { admin: 'Admin', company_admin: 'Admin', manager: 'Manager' }
  return (
    <span className={`inline-flex items-center text-[10px] font-semibold px-2.5 py-1 rounded-full ${cls}`}>
      {labels[r] || 'Member'}
    </span>
  )
}
