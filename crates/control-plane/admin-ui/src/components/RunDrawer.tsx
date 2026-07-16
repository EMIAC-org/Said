import { useEffect, useState } from 'react'
import { apiJson } from '../api'
import { usd, num, timeAgo, firstName, personName } from '../lib/format'
import { STT_PER_HOUR, POLISH_IN_PER_M, POLISH_OUT_PER_M } from '../lib/rates'
import { DrawerClose } from './Drawer'
import { Loading } from './ui'
import type { OrgRun } from '../lib/adminTypes'
import type { DictationDetailItem, DictationTraceStage } from '../types'

const PASTELS = ['var(--tl-thinking)', 'var(--tl-grep)', 'var(--tl-read)', 'var(--tl-edit)', 'var(--tl-done)']

export function RunDrawerHead({ run, onClose }: { run: OrgRun; onClose: () => void }) {
  return (
    <>
      <div>
        <div className="drawer-title mono">{run.run_id}</div>
        <div className="drawer-meta">
          {firstName(run.name || personName(run.lark_name, run.email))} · {run.target_app || 'Unknown'} · {run.mode} · {timeAgo(run.event_at)}
        </div>
      </div>
      <DrawerClose onClick={onClose} />
    </>
  )
}

export function RunDrawerBody({ run, orgId }: { run: OrgRun; orgId: string }) {
  const [detail, setDetail] = useState<DictationDetailItem | null>(null)
  const [loading, setLoading] = useState(true)

  useEffect(() => {
    const rid = run.recording_id || run.run_id
    apiJson<{ item: DictationDetailItem }>(`/v1/orgs/${orgId}/observability/dictation/${rid}`)
      .then(r => setDetail(r.item))
      .catch(() => setDetail(null))
      .finally(() => setLoading(false))
  }, [run, orgId])

  const trace = detail?.dictation_trace_json
  const stages: DictationTraceStage[] =
    trace && 'stages' in trace && Array.isArray(trace.stages) ? trace.stages : []
  const local = !run.speech_cost_usd

  return (
    <>
      {/* Text pipeline */}
      <div className="section-label first">Text pipeline</div>
      {loading ? (
        <Loading />
      ) : detail ? (
        <>
          <div className="textstage">
            <div className="textstage-head">
              <span className="textstage-name">Raw transcript</span>
              <span className="mono" style={{ fontSize: 11, color: 'var(--muted)' }}>{run.word_count ?? 0}w</span>
            </div>
            <div className="textstage-body mono" style={{ fontSize: 12.5 }}>
              {detail.raw_transcript || detail.transcript || '—'}
            </div>
          </div>
          <div className="textstage">
            <div className="textstage-head">
              <span className="textstage-name">Polished output</span>
              <span className="tag ok">final</span>
            </div>
            <div className="textstage-body">{detail.final_text || detail.polished_output || '—'}</div>
          </div>
        </>
      ) : (
        <div className="hint">No stored transcript for this run (history sync may be off, or text was redacted).</div>
      )}

      {/* Trace */}
      {stages.length > 0 && (
        <>
          <div className="section-label">Trace</div>
          <div className="trace">
            {stages.map((s, i) => {
              const color = PASTELS[i % PASTELS.length]
              return (
                <div className="trace-step" key={s.index ?? i}>
                  <span className="trace-node" style={{ background: color }} />
                  <div className="trace-row">
                    <span className="trace-pill" style={{ background: color }}>{s.stage}</span>
                    {s.duration_ms != null && <span className="trace-dur">{s.duration_ms}ms</span>}
                  </div>
                  <div className="trace-fn"><b>{s.component}</b> · <span className="mono">{s.function}</span></div>
                  {(s.reason || s.risk) && (
                    <div className="trace-note">
                      {s.reason}
                      {s.risk ? <span style={{ color: 'var(--warn)' }}> · ⚠ {s.risk}</span> : null}
                    </div>
                  )}
                </div>
              )
            })}
          </div>
        </>
      )}

      {/* Cost breakdown */}
      <div className="section-label">Cost breakdown</div>
      <div className="card card-pad">
        <table className="costtable">
          <tbody>
            <tr>
              <td className="k">STT — {run.speech_provider || 'local'} · {run.speech_model || '—'}</td>
              <td className="v">{local ? '$0.0000 (local)' : usd(run.speech_cost_usd)}</td>
            </tr>
            {run.audio_seconds != null && !local && (
              <tr>
                <td className="k">&nbsp;&nbsp;{run.audio_seconds}s @ ${STT_PER_HOUR}/hr</td>
                <td className="v cost-mut">{usd(run.speech_cost_usd)}</td>
              </tr>
            )}
            {(run.polish_attempts || []).map((a, i) => (
              <tr key={i}>
                <td className="k">
                  Polish — {a.provider}{a.model ? ` · ${a.model}` : ''}
                  <div style={{ color: 'var(--muted-soft)' }}>
                    &nbsp;&nbsp;{num(a.input_tokens ?? 0)} in @ ${POLISH_IN_PER_M}/M · {num(a.output_tokens ?? 0)} out @ ${POLISH_OUT_PER_M}/M
                  </div>
                </td>
                <td className="v">{usd(a.cost_usd)}</td>
              </tr>
            ))}
            <tr className="total">
              <td>Total · coverage {run.cost_coverage}</td>
              <td className="v">{usd(run.total_cost_usd)}</td>
            </tr>
          </tbody>
        </table>
      </div>

      {/* Latency & outcome */}
      <div className="section-label">Latency &amp; outcome</div>
      <div className="kv k4">
        <div className="cell"><div className="k">Total</div><div className="v mono">{run.total_ms ?? '—'}ms</div></div>
        <div className="cell"><div className="k">Transcribe</div><div className="v mono">{run.transcribe_ms ?? '—'}ms</div></div>
        <div className="cell"><div className="k">Polish</div><div className="v mono">{run.polish_ms ?? '—'}ms</div></div>
        <div className="cell">
          <div className="k">Edit after paste</div>
          <div className="v">
            {run.edit_bucket === 'none' || !run.edit_detected ? <span className="tag ok">clean</span>
              : run.edit_bucket === 'light' ? <span className="tag warn">light</span>
              : <span className="tag err">heavy</span>}
          </div>
        </div>
      </div>
    </>
  )
}
