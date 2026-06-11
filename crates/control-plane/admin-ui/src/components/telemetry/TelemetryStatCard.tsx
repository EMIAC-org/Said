import type { LucideIcon } from 'lucide-react'

export function TelemetryStatCard({
  label,
  value,
  sub,
  icon: Icon,
}: {
  label: string
  value: string
  sub?: string
  icon?: LucideIcon
}) {
  return (
    <div className="card p-4">
      <div className="flex items-center gap-2 text-fg-4 mb-2">
        {Icon && <Icon size={14} />}
        <span className="text-[10px] font-semibold uppercase tracking-wider">{label}</span>
      </div>
      <div className="text-[22px] font-semibold text-fg tabular-nums">{value}</div>
      {sub && <div className="text-[11px] text-fg-4 mt-1">{sub}</div>}
    </div>
  )
}
