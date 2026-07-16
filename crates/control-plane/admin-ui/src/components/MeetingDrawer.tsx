import { useEffect, useState } from 'react'
import { apiJson } from '../api'
import { usd, usd2, num } from '../lib/format'
import { MEET_MODEL, MEET_PROVIDER, MEET_IN_PER_M, MEET_CACHE_IN_PER_M, MEET_OUT_PER_M } from '../lib/rates'
import { DrawerClose } from './Drawer'
import { Avatar, Loading } from './ui'
import type { MeetingCostRow } from '../lib/adminTypes'
import type { MeetingDetail } from '../types'

export function MeetingDrawerHead({ row, onClose }: { row: MeetingCostRow; onClose: () => void }) {
  return (
    <>
      <div>
        <div className="drawer-title">{row.title}</div>
        <div className="drawer-meta">hosted by {row.host_name} · {row.participant_count} people</div>
      </div>
      <DrawerClose onClick={onClose} />
    </>
  )
}

export function MeetingDrawerBody({
  row,
  orgId,
  onOpenPerson,
}: {
  row: MeetingCostRow
  orgId: string
  onOpenPerson: (accountId: string) => void
}) {
  const [detail, setDetail] = useState<MeetingDetail | null>(null)
  const [loading, setLoading] = useState(true)

  useEffect(() => {
    apiJson<MeetingDetail>(`/v1/meetings/${row.id}`)
      .then(setDetail)
      .catch(() => setDetail(null))
      .finally(() => setLoading(false))
  }, [row.id, orgId])

  const nonCached = Math.max(0, row.input_tokens - row.cached_input_tokens)
  const inCost = (nonCached / 1e6) * MEET_IN_PER_M + (row.cached_input_tokens / 1e6) * MEET_CACHE_IN_PER_M
  const outCost = (row.output_tokens / 1e6) * MEET_OUT_PER_M

  return (
    <>
      <div className="section-label first">AI summary cost — {MEET_PROVIDER} {MEET_MODEL}</div>
      <div className="card card-pad">
        <table className="costtable">
          <tbody>
            <tr>
              <td className="k">Input — {num(nonCached)} tokens @ ${MEET_IN_PER_M}/M</td>
              <td className="v">{usd((nonCached / 1e6) * MEET_IN_PER_M)}</td>
            </tr>
            {row.cached_input_tokens > 0 && (
              <tr>
                <td className="k">Cached input — {num(row.cached_input_tokens)} @ ${MEET_CACHE_IN_PER_M}/M</td>
                <td className="v">{usd((row.cached_input_tokens / 1e6) * MEET_CACHE_IN_PER_M)}</td>
              </tr>
            )}
            <tr>
              <td className="k">Output — {num(row.output_tokens)} tokens @ ${MEET_OUT_PER_M}/M</td>
              <td className="v">{usd(outCost)}</td>
            </tr>
            <tr className="total">
              <td>Meeting total</td>
              <td className="v">{usd2(row.cost_usd || inCost + outCost)}</td>
            </tr>
          </tbody>
        </table>
      </div>

      <div className="section-label">Participants</div>
      <div className="card">
        {loading ? <Loading /> : (
          <table>
            <tbody>
              {(detail?.participants ?? []).map((p, i) => {
                const nm = p.lark_name || p.name || p.account_id
                return (
                  <tr key={p.id} className="clickable" onClick={() => onOpenPerson(p.account_id)}>
                    <td>
                      <div className="person-cell">
                        <Avatar name={nm} size={26} />
                        <div><div className="nm">{nm}</div><div className="em">{i === 0 ? 'host' : 'participant'}</div></div>
                      </div>
                    </td>
                    <td className="r" style={{ color: 'var(--muted-soft)' }}>›</td>
                  </tr>
                )
              })}
              {!loading && (detail?.participants?.length ?? 0) === 0 && (
                <tr><td style={{ color: 'var(--muted)', fontSize: 13 }}>No participants recorded.</td></tr>
              )}
            </tbody>
          </table>
        )}
      </div>

      {detail?.summary && (
        <>
          <div className="section-label">Summary</div>
          <div className="textstage">
            <div className="textstage-head">
              <span className="textstage-name">Generated summary</span>
              <span className="chip mono">{MEET_MODEL}</span>
            </div>
            <div className="textstage-body">{detail.summary}</div>
          </div>
        </>
      )}
    </>
  )
}
