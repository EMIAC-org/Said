import { useEffect, useState } from 'react'
import { useSearchParams } from 'react-router'
import { BarChart3, Clock, Mic, ThumbsUp, Zap } from 'lucide-react'
import { apiJson } from '../api'
import { useAuth } from '../hooks/useAuth'
import { TelemetryTabs } from '../components/telemetry/TelemetryTabs'
import { TelemetryStatCard } from '../components/telemetry/TelemetryStatCard'
import { pct, ms } from '../components/telemetry/format'
import { ErrorBox, Loading } from '../components/States'

interface TelemetryAnalytics {
  window_days: number
  usage: {
    dau: number
    wau: number
    completed_runs: number
    audio_minutes: number
    by_mode: { mode: string; count: number }[]
    by_target_app: { target_app: string | null; count: number }[]
  }
  quality: {
    acceptance_rate: number
    edit_rate: number
    heavy_edit_rate: number
    fallback_rate: number
    learning_candidate_rate: number
    learning_success_rate: number
  }
  latency_ms: {
    total_p50: number | null
    total_p95: number | null
    transcribe_p50: number | null
    transcribe_p95: number | null
  }
  stt?: {
    by_provider_path: { stt_provider: string; stt_path: string; count: number }[]
    by_provider: { stt_provider: string; count: number; share: number }[]
    total_tagged: number
  }
}

export function TelemetryPage() {
  const { org } = useAuth()
  const [searchParams, setSearchParams] = useSearchParams()
  const days = Number(searchParams.get('days') || '30')
  const [data, setData] = useState<TelemetryAnalytics | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')

  useEffect(() => {
    const orgId = org?.org?.id
    if (!orgId) {
      setLoading(false)
      return
    }
    setLoading(true)
    apiJson<TelemetryAnalytics>(`/v1/orgs/${orgId}/telemetry?days=${days}`)
      .then(setData)
      .catch(e => setError(e.message))
      .finally(() => setLoading(false))
  }, [org, days])

  if (!org?.org?.id) {
    return (
      <div className="card p-6 text-[13px] text-fg-3">
        Select an organization workspace to view desktop telemetry.
      </div>
    )
  }

  return (
    <>
      <div className="mb-4 flex items-end justify-between gap-4 flex-wrap">
        <div>
          <h1 className="text-[15px] font-semibold text-fg">Desktop Telemetry</h1>
          <p className="text-[12px] text-fg-4 mt-1">
            Privacy-safe per-run analytics from signed-in desktops. No transcript text is stored.
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

      <TelemetryTabs />

      {loading ? (
        <Loading />
      ) : error ? (
        <ErrorBox title="Failed to load telemetry" message={error} />
      ) : !data ? null : (
        <>
          <div className="grid grid-cols-3 gap-3 mb-4">
            <TelemetryStatCard
              label="Completed runs"
              value={data.usage.completed_runs.toLocaleString()}
              icon={Mic}
              sub={`${data.usage.audio_minutes} min dictated`}
            />
            <TelemetryStatCard
              label="DAU / WAU"
              value={`${data.usage.dau} / ${data.usage.wau}`}
              icon={BarChart3}
              sub="Active dictation users"
            />
            <TelemetryStatCard
              label="Acceptance rate"
              value={pct(data.quality.acceptance_rate)}
              icon={ThumbsUp}
              sub="No meaningful edit after paste"
            />
            <TelemetryStatCard
              label="Total latency p50"
              value={ms(data.latency_ms.total_p50)}
              icon={Clock}
              sub={`p95 ${ms(data.latency_ms.total_p95)}`}
            />
            <TelemetryStatCard
              label="Edit rate"
              value={pct(data.quality.edit_rate)}
              icon={Zap}
              sub={`Heavy ${pct(data.quality.heavy_edit_rate)}`}
            />
            <TelemetryStatCard
              label="Fallback rate"
              value={pct(data.quality.fallback_rate)}
              icon={Zap}
              sub="Clipboard or HTTP STT fallback"
            />
          </div>

          <div className="grid grid-cols-2 gap-4">
            <div className="card p-4">
              <div className="text-[10px] font-semibold text-fg-4 uppercase tracking-wider mb-3">
                Runs by mode
              </div>
              {data.usage.by_mode.length === 0 ? (
                <div className="text-[12px] text-fg-4">No runs yet.</div>
              ) : (
                <div className="space-y-2">
                  {data.usage.by_mode.map(row => (
                    <div key={row.mode} className="flex items-center justify-between text-[12px]">
                      <span className="text-fg-2 font-mono">{row.mode}</span>
                      <span className="text-fg tabular-nums">{row.count}</span>
                    </div>
                  ))}
                </div>
              )}
            </div>

            <div className="card p-4">
              <div className="text-[10px] font-semibold text-fg-4 uppercase tracking-wider mb-3">
                Top target apps
              </div>
              {data.usage.by_target_app.length === 0 ? (
                <div className="text-[12px] text-fg-4">No app breakdown yet.</div>
              ) : (
                <div className="space-y-2">
                  {data.usage.by_target_app.map((row, i) => (
                    <div key={i} className="flex items-center justify-between text-[12px]">
                      <span className="text-fg-2 truncate max-w-[70%]">
                        {row.target_app || 'Unknown'}
                      </span>
                      <span className="text-fg tabular-nums">{row.count}</span>
                    </div>
                  ))}
                </div>
              )}
            </div>

            <div className="card p-4">
              <div className="text-[10px] font-semibold text-fg-4 uppercase tracking-wider mb-3">
                STT provider mix
              </div>
              {!data.stt?.by_provider?.length ? (
                <div className="text-[12px] text-fg-4">No STT-tagged runs yet.</div>
              ) : (
                <div className="space-y-2">
                  {data.stt.by_provider.map(row => (
                    <div key={row.stt_provider} className="flex items-center justify-between text-[12px]">
                      <span className="text-fg-2 font-mono">{row.stt_provider}</span>
                      <span className="text-fg tabular-nums">
                        {row.count} ({row.share}%)
                      </span>
                    </div>
                  ))}
                  {data.stt.by_provider_path.slice(0, 6).map(row => (
                    <div
                      key={`${row.stt_provider}-${row.stt_path}`}
                      className="flex items-center justify-between text-[11px] text-fg-4"
                    >
                      <span className="font-mono">
                        {row.stt_provider} · {row.stt_path}
                      </span>
                      <span className="tabular-nums">{row.count}</span>
                    </div>
                  ))}
                </div>
              )}
            </div>

            <div className="card p-4">
              <div className="text-[10px] font-semibold text-fg-4 uppercase tracking-wider mb-3">
                Learning funnel
              </div>
              <div className="grid grid-cols-3 gap-4 text-[12px]">
                <div>
                  <div className="text-fg-4">Candidate rate</div>
                  <div className="text-fg font-semibold tabular-nums mt-1">
                    {pct(data.quality.learning_candidate_rate)}
                  </div>
                </div>
                <div>
                  <div className="text-fg-4">Success rate</div>
                  <div className="text-fg font-semibold tabular-nums mt-1">
                    {pct(data.quality.learning_success_rate)}
                  </div>
                </div>
                <div>
                  <div className="text-fg-4">STT p50 / p95</div>
                  <div className="text-fg font-semibold tabular-nums mt-1">
                    {ms(data.latency_ms.transcribe_p50)} / {ms(data.latency_ms.transcribe_p95)}
                  </div>
                </div>
              </div>
            </div>
          </div>
        </>
      )}
    </>
  )
}
