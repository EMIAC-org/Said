export function usd(n: number | null | undefined): string {
  const v = n ?? 0
  const dp = Math.abs(v) < 1 ? 4 : 2
  return '$' + v.toLocaleString('en-US', { minimumFractionDigits: dp, maximumFractionDigits: dp })
}

export function usd2(n: number | null | undefined): string {
  return '$' + (n ?? 0).toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 })
}

export function num(n: number | null | undefined): string {
  return (n ?? 0).toLocaleString('en-US')
}

export function initials(name: string): string {
  const parts = (name || '?').trim().split(/\s+/)
  if (parts.length === 1) return parts[0].charAt(0).toUpperCase()
  return (parts[0].charAt(0) + parts[parts.length - 1].charAt(0)).toUpperCase()
}

/** Platform → human label + glyph. OS version is not stored; derived from platform. */
export function osLabel(platform?: string | null): string {
  if (!platform) return 'Unknown'
  const p = platform.toLowerCase()
  if (p.includes('mac') || p === 'darwin') return 'macOS'
  if (p.includes('win')) return 'Windows'
  return platform
}

export function osGlyph(platform?: string | null): string {
  const p = (platform || '').toLowerCase()
  if (p.includes('mac') || p === 'darwin') return ''
  if (p.includes('win')) return '⊞'
  return '•'
}

export function timeAgo(iso?: string | null): string {
  if (!iso) return '—'
  const then = new Date(iso).getTime()
  if (Number.isNaN(then)) return '—'
  const s = Math.max(0, Math.floor((Date.now() - then) / 1000))
  if (s < 60) return 'just now'
  const m = Math.floor(s / 60)
  if (m < 60) return `${m}m ago`
  const h = Math.floor(m / 60)
  if (h < 24) return `${h}h ago`
  const d = Math.floor(h / 24)
  return `${d}d ago`
}

export function shortDate(iso?: string | null): string {
  if (!iso) return '—'
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return '—'
  return d.toLocaleDateString('en-US', { month: 'short', day: 'numeric' })
}

export function firstName(name?: string | null): string {
  return (name || '').trim().split(/\s+/)[0] || '—'
}

/** Compact identifiers in dense tables; the full value remains available in a title. */
export function shortId(value: string, head = 8, tail = 4): string {
  if (value.length <= head + tail + 1) return value
  return `${value.slice(0, head)}…${value.slice(-tail)}`
}

/** Display name for a person from their lark_name / email. */
export function personName(larkName?: string | null, email?: string | null): string {
  return larkName || email?.split('@')[0] || 'Unknown'
}
