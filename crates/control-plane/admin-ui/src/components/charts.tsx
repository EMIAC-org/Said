import type { ReactNode } from 'react'

// ── Sparkline — tiny inline trend line ──────────────────────────────────────
export function Sparkline({
  points,
  width = 80,
  height = 24,
  color = 'var(--color-accent)',
  fill = true,
}: {
  points: number[]
  width?: number
  height?: number
  color?: string
  fill?: boolean
}) {
  if (!points.length) return null
  const max = Math.max(...points, 1)
  const min = Math.min(...points, 0)
  const span = max - min || 1
  const step = width / Math.max(points.length - 1, 1)
  const coords = points.map((p, i) => [i * step, height - 2 - ((p - min) / span) * (height - 4)])
  const line = coords.map(([x, y]) => `${x.toFixed(1)},${y.toFixed(1)}`).join(' ')
  const area = `0,${height} ${line} ${width},${height}`
  const id = `spk${Math.round(points.reduce((a, b) => a + b, 0))}-${points.length}`
  return (
    <svg width={width} height={height} viewBox={`0 0 ${width} ${height}`} preserveAspectRatio="none" className="overflow-visible">
      {fill && (
        <>
          <defs>
            <linearGradient id={id} x1="0" y1="0" x2="0" y2="1">
              <stop offset="0%" stopColor={color} stopOpacity="0.22" />
              <stop offset="100%" stopColor={color} stopOpacity="0" />
            </linearGradient>
          </defs>
          <polygon points={area} fill={`url(#${id})`} />
        </>
      )}
      <polyline points={line} fill="none" stroke={color} strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  )
}

// ── KPI / metric tile ───────────────────────────────────────────────────────
export function MetricCard({
  label,
  value,
  sub,
  delta,
  icon,
  accent = false,
}: {
  label: string
  value: ReactNode
  sub?: ReactNode
  delta?: number // signed fraction, e.g. 0.12 = +12%
  icon?: ReactNode
  accent?: boolean
}) {
  const up = (delta ?? 0) >= 0
  return (
    <div className={accent ? 'card-gradient' : 'card'}>
      <div className="flex items-center justify-between mb-2.5">
        <span className="text-[10px] font-semibold text-fg-4 uppercase tracking-wider">{label}</span>
        {icon && <span className="text-fg-4">{icon}</span>}
      </div>
      <div className="flex items-baseline gap-2">
        <span className="text-[26px] font-semibold tracking-tighter leading-none tabular-nums">{value}</span>
        {delta !== undefined && (
          <span className={`text-[10px] font-semibold px-1.5 py-0.5 rounded ${up ? 'text-ok bg-ok-bg' : 'text-live bg-live-bg'}`}>
            {up ? '▲' : '▼'} {Math.abs(Math.round((delta ?? 0) * 100))}%
          </span>
        )}
      </div>
      {sub && <div className="text-[11px] text-fg-4 mt-1.5">{sub}</div>}
    </div>
  )
}

// ── Grouped bar chart (daily volume: total vs accepted) ─────────────────────
export function GroupedBars({
  data,
  height = 150,
}: {
  data: { day: string; count: number; accepted: number }[]
  height?: number
}) {
  const max = Math.max(...data.map((d) => d.count), 1)
  return (
    <div className="flex items-end gap-3" style={{ height }}>
      {data.map((d) => (
        <div key={d.day} className="flex-1 flex flex-col items-center gap-1.5 min-w-0">
          <div className="w-full flex items-end justify-center gap-[3px]" style={{ height: height - 20 }}>
            <div
              className="w-1/2 rounded-t-[3px] bg-surface-4 relative group"
              style={{ height: `${(d.count / max) * 100}%` }}
              title={`${d.count} polishes`}
            />
            <div
              className="w-1/2 rounded-t-[3px] bg-accent relative"
              style={{ height: `${(d.accepted / max) * 100}%`, boxShadow: '0 0 8px var(--color-accent-glow)' }}
              title={`${d.accepted} accepted`}
            />
          </div>
          <span className="text-[9px] text-fg-4 font-medium">{d.day}</span>
        </div>
      ))}
    </div>
  )
}

// ── Donut (model / source split) ────────────────────────────────────────────
const DONUT_COLORS = ['var(--color-accent)', 'var(--color-speaker-2)', 'var(--color-speaker-3)', 'var(--color-speaker-5)']
export function Donut({
  data,
  size = 120,
}: {
  data: { label: string; value: number }[]
  size?: number
}) {
  const total = data.reduce((a, d) => a + d.value, 0) || 1
  const r = size / 2 - 10
  const c = 2 * Math.PI * r
  let offset = 0
  return (
    <div className="flex items-center gap-4">
      <svg width={size} height={size} viewBox={`0 0 ${size} ${size}`} className="-rotate-90">
        {data.map((d, i) => {
          const frac = d.value / total
          const dash = frac * c
          const seg = (
            <circle
              key={i}
              cx={size / 2}
              cy={size / 2}
              r={r}
              fill="none"
              stroke={DONUT_COLORS[i % DONUT_COLORS.length]}
              strokeWidth="11"
              strokeDasharray={`${dash} ${c - dash}`}
              strokeDashoffset={-offset}
              strokeLinecap="butt"
            />
          )
          offset += dash
          return seg
        })}
      </svg>
      <div className="space-y-1.5">
        {data.map((d, i) => (
          <div key={d.label} className="flex items-center gap-2 text-[11px]">
            <span className="w-2.5 h-2.5 rounded-sm" style={{ background: DONUT_COLORS[i % DONUT_COLORS.length] }} />
            <span className="text-fg-3">{d.label}</span>
            <span className="text-fg-4 tabular-nums ml-1">{Math.round((d.value / total) * 100)}%</span>
          </div>
        ))}
      </div>
    </div>
  )
}

// ── Horizontal labelled bar ─────────────────────────────────────────────────
export function Bar({ label, value, total, color = 'bg-accent', suffix }: { label: string; value: number; total: number; color?: string; suffix?: string }) {
  const pct = total > 0 ? (value / total) * 100 : 0
  return (
    <div>
      <div className="flex items-center justify-between mb-1.5">
        <span className="text-[12px] text-fg-3">{label}</span>
        <span className="text-[12px] font-medium tabular-nums">{suffix ?? value}</span>
      </div>
      <div className="h-2 bg-border-light rounded-full overflow-hidden">
        <div className={`h-full rounded-full ${color} transition-all`} style={{ width: `${Math.max(pct, 2)}%` }} />
      </div>
    </div>
  )
}

// ── Acceptance gauge (semicircle) ───────────────────────────────────────────
export function Gauge({ value, label }: { value: number; label?: string }) {
  const pct = Math.round(value * 100)
  const r = 80
  const circ = Math.PI * r
  const offset = circ - circ * value
  return (
    <div className="flex flex-col items-center">
      <svg width={170} height={96} viewBox="0 0 200 105">
        <path d="M 20 95 A 80 80 0 0 1 180 95" fill="none" stroke="var(--color-border)" strokeWidth="12" strokeLinecap="round" />
        <path
          d="M 20 95 A 80 80 0 0 1 180 95"
          fill="none"
          stroke="var(--color-accent)"
          strokeWidth="12"
          strokeLinecap="round"
          strokeDasharray={circ}
          strokeDashoffset={offset}
          style={{ filter: 'drop-shadow(0 0 6px var(--color-accent-glow))' }}
        />
      </svg>
      <div className="text-center -mt-3">
        <div className="text-2xl font-semibold tracking-tight tabular-nums">{pct}%</div>
        {label && <div className="text-[10px] text-fg-4">{label}</div>}
      </div>
    </div>
  )
}

// ── Stage timeline (runtime_stage_events for one polish) ────────────────────
export function StageTimeline({
  stages,
}: {
  stages: {
    stage: string
    status: string
    latency_ms: number | null
    error_kind?: string | null
    metadata_json?: Record<string, unknown>
  }[]
}) {
  const dot: Record<string, string> = { ok: 'bg-ok', error: 'bg-live', warning: 'bg-warn' }
  const txt: Record<string, string> = { ok: 'text-fg-3', error: 'text-live', warning: 'text-warn' }
  return (
    <div className="relative pl-4">
      <div className="absolute left-[5px] top-1.5 bottom-1.5 w-px bg-border" />
      <div className="space-y-2.5">
        {stages.map((s, i) => {
          const meta = s.metadata_json
          const profileMarkdown =
            s.stage === 'prompt_built' && meta && typeof meta.profile_markdown === 'string'
              ? meta.profile_markdown
              : null
          const profileChars =
            s.stage === 'prompt_built' && meta && typeof meta.profile_chars === 'number'
              ? meta.profile_chars
              : null
          const profileHash =
            s.stage === 'prompt_built' && meta && typeof meta.profile_hash === 'string'
              ? meta.profile_hash
              : null
          return (
            <div key={i} className="relative">
              <div className="flex items-center gap-3">
                <span
                  className={`absolute -left-4 w-[7px] h-[7px] rounded-full ${dot[s.status] || 'bg-fg-4'}`}
                />
                <span className={`text-[12px] font-mono ${txt[s.status] || 'text-fg-3'}`}>{s.stage}</span>
                {s.error_kind && (
                  <span className="text-[10px] text-live bg-live-bg px-1.5 py-0.5 rounded">{s.error_kind}</span>
                )}
                <span className="ml-auto text-[11px] text-fg-4 tabular-nums">
                  {s.latency_ms != null ? `${s.latency_ms} ms` : '—'}
                </span>
              </div>
              {s.stage === 'prompt_built' && (profileChars != null || profileHash) ? (
                <div className="ml-0 mt-1.5 pl-0 text-[10px] text-fg-4 font-mono space-y-1">
                  {profileChars != null ? <div>{profileChars} profile chars</div> : null}
                  {profileHash ? <div className="truncate">hash {profileHash.slice(0, 16)}…</div> : null}
                  {profileMarkdown ? (
                    <pre className="text-[10px] leading-relaxed whitespace-pre-wrap break-words font-mono bg-surface-3 rounded p-2 border border-border-light max-h-[10rem] overflow-auto mt-1">
                      {profileMarkdown.length > 600
                        ? `${profileMarkdown.slice(0, 600)}…`
                        : profileMarkdown}
                    </pre>
                  ) : null}
                </div>
              ) : null}
            </div>
          )
        })}
      </div>
    </div>
  )
}

// ── Small reusable status badge ─────────────────────────────────────────────
export function Badge({ value, tone }: { value: string; tone?: 'ok' | 'warn' | 'live' | 'info' | 'muted' }) {
  const map: Record<string, string> = {
    ok: 'text-ok bg-ok-bg',
    warn: 'text-warn bg-warn-bg',
    live: 'text-live bg-live-bg',
    info: 'text-info bg-info-bg',
    muted: 'text-fg-3 bg-surface-4',
  }
  const auto =
    tone ||
    (['completed', 'approved', 'active', 'safe', 'learned'].includes(value)
      ? 'ok'
      : ['failed', 'blocked', 'rejected', 'unsafe'].includes(value)
        ? 'live'
        : ['draft', 'unknown', 'pending', 'archived'].includes(value)
          ? 'warn'
          : 'muted')
  return <span className={`text-[10px] font-semibold px-2 py-0.5 rounded-full uppercase tracking-wide ${map[auto]}`}>{value}</span>
}
