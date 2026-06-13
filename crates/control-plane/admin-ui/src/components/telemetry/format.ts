export function pct(n: number) {
  return `${Math.round(n * 1000) / 10}%`
}

export function ms(n: number | null | undefined) {
  if (n == null) return '—'
  return `${Math.round(n)} ms`
}

export function relTime(iso: string | null | undefined): string {
  if (!iso) return '—'
  const diff = Date.now() - new Date(iso).getTime()
  const min = Math.floor(diff / 60_000)
  if (min < 1) return 'Just now'
  if (min < 60) return `${min}m ago`
  const hr = Math.floor(min / 60)
  if (hr < 24) return `${hr}h ago`
  return `${Math.floor(hr / 24)}d ago`
}
