const AV_COLORS = [
  '#6366f1', '#8b5cf6', '#ec4899', '#f43f5e', '#f97316',
  '#eab308', '#22c55e', '#14b8a6', '#06b6d4', '#3b82f6',
]

function nameHash(n: string): number {
  let h = 0
  for (let i = 0; i < n.length; i++) h = n.charCodeAt(i) + ((h << 5) - h)
  return Math.abs(h)
}

export function avatarColor(name: string): string {
  return AV_COLORS[nameHash(name || '?') % AV_COLORS.length]
}

export function avatarInitials(name: string): string {
  if (!name) return '?'
  const p = name.trim().split(/\s+/)
  return p.length === 1 ? p[0][0].toUpperCase() : (p[0][0] + p[p.length - 1][0]).toUpperCase()
}

export function formatDate(s?: string | null): string {
  if (!s) return '--'
  return new Date(s).toLocaleDateString('en-US', { month: 'short', day: 'numeric', year: 'numeric' })
}

export function formatTime(s?: string | null): string {
  if (!s) return ''
  return new Date(s).toLocaleTimeString('en-US', { hour: 'numeric', minute: '2-digit' })
}

export function timeAgo(s?: string | null): string {
  if (!s) return '--'
  const m = Math.floor((Date.now() - new Date(s).getTime()) / 60000)
  if (m < 1) return 'Just now'
  if (m < 60) return m + 'm ago'
  const h = Math.floor(m / 60)
  if (h < 24) return h + 'h ago'
  const d = Math.floor(h / 24)
  return d < 7 ? d + 'd ago' : formatDate(s)
}

export function duration(start?: string | null, end?: string | null): string {
  if (!start || !end) return '--'
  const m = Math.round((new Date(end).getTime() - new Date(start).getTime()) / 60000)
  if (m < 60) return m + ' min'
  return Math.floor(m / 60) + 'h ' + (m % 60) + 'm'
}
