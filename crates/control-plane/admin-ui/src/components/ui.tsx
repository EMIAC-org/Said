import type { ReactNode } from 'react'
import { initials } from '../lib/format'

/* ── Avatar ─────────────────────────────────────────────────────── */
export function Avatar({ name, size = 28 }: { name: string; size?: number }) {
  return (
    <div
      className="avatar"
      style={{ width: size, height: size, fontSize: Math.round(size * 0.42), borderRadius: size >= 44 ? 14 : 999 }}
    >
      {initials(name)}
    </div>
  )
}

/* ── Stat tile ──────────────────────────────────────────────────── */
export function StatTile({
  label,
  value,
  sub,
  delta,
}: {
  label: string
  value: ReactNode
  sub?: ReactNode
  delta?: { dir: 'up' | 'down'; val: string }
}) {
  return (
    <div className="card stat">
      <div className="label">{label}</div>
      <div className="val tnum">{value}</div>
      {(sub || delta) && (
        <div className="sub">
          {delta && <span className={`delta ${delta.dir}`}>{delta.val}</span>}
          {sub}
        </div>
      )}
    </div>
  )
}

/* ── Dense table text ───────────────────────────────────────────── */
export function TruncatedChip({
  value,
  mono = false,
}: {
  value: string
  mono?: boolean
}) {
  return <span className={`chip truncate${mono ? ' mono' : ''}`} title={value}>{value}</span>
}

/* ── Sparkline (area + line) ───────────────────────────────────── */
export function Sparkline({ values, height = 120 }: { values: number[]; height?: number }) {
  const w = 560
  const h = height
  if (values.length < 2) values = [0, 0]
  const max = Math.max(...values)
  const min = Math.min(...values)
  const dx = w / (values.length - 1)
  const pts = values.map((v, i) => {
    const y = h - ((v - min) / (max - min || 1)) * (h - 16) - 8
    return `${(i * dx).toFixed(1)},${y.toFixed(1)}`
  })
  const line = pts.join(' ')
  const area = `0,${h} ${line} ${w},${h}`
  const lastY = pts[pts.length - 1].split(',')[1]
  return (
    <svg viewBox={`0 0 ${w} ${h}`} preserveAspectRatio="none" style={{ width: '100%', height, display: 'block' }}>
      {[0, 0.25, 0.5, 0.75, 1].map(f => (
        <line key={f} className="grid-line" x1="0" y1={(f * h).toFixed(0)} x2={w} y2={(f * h).toFixed(0)} />
      ))}
      <polygon className="area" points={area} />
      <polyline className="line" points={line} />
      <circle cx={w} cy={lastY} r="3.5" fill="var(--surface-card)" stroke="var(--primary)" strokeWidth="2" />
    </svg>
  )
}

/* ── Split bar + legend ─────────────────────────────────────────── */
export function SplitBar({ segments }: { segments: { pct: number; color: string }[] }) {
  return (
    <div className="splitbar">
      {segments.map((s, i) => (
        <span key={i} style={{ width: `${Math.max(0, s.pct)}%`, background: s.color }} />
      ))}
    </div>
  )
}

/* ── States ─────────────────────────────────────────────────────── */
export function Loading() {
  return (
    <div className="state">
      <div className="spinner" /> Loading…
    </div>
  )
}

export function ErrorBox({ title, message }: { title: string; message: string }) {
  return (
    <div className="errbox">
      <h3>{title}</h3>
      <p>{message}</p>
    </div>
  )
}

export function Empty({ title, message }: { title: string; message?: string }) {
  return (
    <div className="state" style={{ flexDirection: 'column', gap: 4 }}>
      <div style={{ color: 'var(--ink)', fontSize: 14, fontWeight: 500 }}>{title}</div>
      {message && <div>{message}</div>}
    </div>
  )
}
