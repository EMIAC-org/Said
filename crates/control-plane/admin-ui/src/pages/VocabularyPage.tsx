import { useEffect, useState } from 'react'
import { BookOpenText, Check, CircleSlash, Plus, RefreshCw, UploadCloud, X } from 'lucide-react'
import { apiJson } from '../api'
import { Empty, ErrorBox, Loading } from '../components/States'
import { formatDate } from '../utils'
import { useAuth } from '../hooks/useAuth'
import type { OrgVocabAlias, OrgVocabRelease, OrgVocabSuggestion, OrgVocabTerm } from '../types'

type Tab = 'bucket' | 'suggestions' | 'releases'

const inputClass = 'h-9 rounded-lg bg-surface-4/60 border border-border px-3 text-[12px] outline-none focus:border-accent/60'
const buttonClass = 'h-9 inline-flex items-center justify-center gap-2 rounded-lg bg-[hsl(0_0%_98%)] px-3 text-[12px] font-semibold text-[hsl(240_8%_8%)] hover:opacity-90 disabled:opacity-50'
const ghostButtonClass = 'h-8 inline-flex items-center justify-center gap-1.5 rounded-md bg-surface-4 px-2.5 text-[11px] font-medium text-fg-3 hover:text-fg'

function StatusBadge({ value }: { value: string }) {
  const cls =
    value === 'approved' ? 'text-ok bg-ok-bg' :
    value === 'blocked' || value === 'rejected' ? 'text-live bg-live-bg' :
    value === 'pending' ? 'text-warn bg-warn-bg' :
    'text-fg-3 bg-surface-4'
  return <span className={`text-[10px] font-semibold px-2 py-0.5 rounded-full uppercase tracking-wide ${cls}`}>{value}</span>
}

export function VocabularyPage() {
  const { org } = useAuth()
  const orgId = org?.org?.id
  const [tab, setTab] = useState<Tab>('bucket')
  const [terms, setTerms] = useState<OrgVocabTerm[]>([])
  const [aliases, setAliases] = useState<OrgVocabAlias[]>([])
  const [suggestions, setSuggestions] = useState<OrgVocabSuggestion[]>([])
  const [releases, setReleases] = useState<OrgVocabRelease[]>([])
  const [loading, setLoading] = useState(true)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [termForm, setTermForm] = useState({ term: '', term_type: 'brand', priority: '0', status: 'approved' })
  const [aliasForm, setAliasForm] = useState({ transcript_form: '', correct_form: '', status: 'approved' })

  async function loadAll() {
    if (!orgId) return
    setLoading(true)
    setError('')
    try {
      const [termData, aliasData, suggestionData, releaseData] = await Promise.all([
        apiJson<{ terms: OrgVocabTerm[] }>(`/v1/orgs/${orgId}/vocab/terms?status=all`),
        apiJson<{ aliases: OrgVocabAlias[] }>(`/v1/orgs/${orgId}/vocab/aliases?status=all`),
        apiJson<{ suggestions: OrgVocabSuggestion[] }>(`/v1/orgs/${orgId}/vocab/suggestions?status=all`),
        apiJson<{ releases: OrgVocabRelease[] }>(`/v1/orgs/${orgId}/vocab/releases`),
      ])
      setTerms(termData.terms || [])
      setAliases(aliasData.aliases || [])
      setSuggestions(suggestionData.suggestions || [])
      setReleases(releaseData.releases || [])
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to load vocabulary')
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => { void loadAll() }, [orgId])

  async function addTerm() {
    if (!orgId || !termForm.term.trim()) return
    setBusy(true)
    try {
      await apiJson(`/v1/orgs/${orgId}/vocab/terms`, {
        method: 'POST',
        body: JSON.stringify({
          term: termForm.term,
          term_type: termForm.term_type,
          priority: Number(termForm.priority) || 0,
          status: termForm.status,
        }),
      })
      setTermForm(f => ({ ...f, term: '' }))
      await loadAll()
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to add term')
    } finally {
      setBusy(false)
    }
  }

  async function addAlias() {
    if (!orgId || !aliasForm.transcript_form.trim() || !aliasForm.correct_form.trim()) return
    setBusy(true)
    try {
      await apiJson(`/v1/orgs/${orgId}/vocab/aliases`, {
        method: 'POST',
        body: JSON.stringify(aliasForm),
      })
      setAliasForm(f => ({ ...f, transcript_form: '', correct_form: '' }))
      await loadAll()
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to add alias')
    } finally {
      setBusy(false)
    }
  }

  async function publish() {
    if (!orgId) return
    setBusy(true)
    try {
      await apiJson(`/v1/orgs/${orgId}/vocab/publish`, {
        method: 'POST',
        body: JSON.stringify({ notes: 'Published from admin UI' }),
      })
      await loadAll()
      setTab('releases')
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to publish')
    } finally {
      setBusy(false)
    }
  }

  async function aggregate() {
    if (!orgId) return
    setBusy(true)
    try {
      await apiJson(`/v1/orgs/${orgId}/vocab/suggestions/aggregate`, { method: 'POST' })
      await loadAll()
      setTab('suggestions')
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to aggregate suggestions')
    } finally {
      setBusy(false)
    }
  }

  async function suggestionAction(id: string, action: 'approve' | 'reject' | 'block') {
    if (!orgId) return
    setBusy(true)
    try {
      await apiJson(`/v1/orgs/${orgId}/vocab/suggestions/${id}`, {
        method: 'PATCH',
        body: JSON.stringify({ action }),
      })
      await loadAll()
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to update suggestion')
    } finally {
      setBusy(false)
    }
  }

  if (loading) return <Loading />
  if (!orgId) return <ErrorBox title="No workspace" message="Connect to an organization first." />

  const pending = suggestions.filter(s => s.status === 'pending').length
  const latest = releases[0]

  return (
    <>
      <div className="mb-5 flex items-end justify-between gap-4">
        <div>
          <h1 className="text-xl font-semibold tracking-tight">Vocabulary</h1>
          <p className="text-[12px] text-fg-4 mt-0.5">
            Company bucket for day-one dictation accuracy · {terms.filter(t => t.status === 'approved').length} approved terms · {aliases.filter(a => a.status === 'approved').length} aliases
            {latest ? ` · published v${latest.version}` : ''}
          </p>
        </div>
        <div className="flex gap-2">
          <button className={ghostButtonClass} disabled={busy} onClick={() => void aggregate()}><RefreshCw size={13} /> Aggregate</button>
          <button className={buttonClass} disabled={busy} onClick={() => void publish()}><UploadCloud size={14} /> Publish</button>
        </div>
      </div>

      {error && <div className="mb-4"><ErrorBox title="Vocabulary action failed" message={error} /></div>}

      <div className="mb-4 flex gap-1.5">
        {([
          ['bucket', 'Company Bucket'],
          ['suggestions', `Suggestions${pending ? ` (${pending})` : ''}`],
          ['releases', 'Releases'],
        ] as [Tab, string][]).map(([id, label]) => (
          <button
            key={id}
            onClick={() => setTab(id)}
            className={`h-8 rounded-lg px-3 text-[12px] font-medium ${tab === id ? 'bg-surface-4 text-fg' : 'text-fg-4 hover:text-fg hover:bg-surface-4/40'}`}
          >
            {label}
          </button>
        ))}
      </div>

      {tab === 'bucket' && (
        <div className="grid grid-cols-[minmax(0,1fr)_340px] gap-4">
          <div className="space-y-4">
            <section className="card !p-0 overflow-hidden">
              <div className="px-5 py-3 border-b border-border flex items-center justify-between">
                <h2 className="text-[13px] font-semibold">Company Terms</h2>
                <span className="text-[11px] text-fg-4">{terms.length} total</span>
              </div>
              {!terms.length ? <div className="p-5"><Empty title="No terms yet" message="Add approved company vocabulary such as product names, acronyms, and internal tools." /></div> : (
                <table className="w-full">
                  <thead><tr>{['Term', 'Type', 'Priority', 'Status', 'Updated'].map(h => <th key={h} className="text-[10px] font-medium text-fg-4 text-left px-5 py-3 border-b border-border uppercase tracking-wider">{h}</th>)}</tr></thead>
                  <tbody>{terms.map(t => (
                    <tr key={t.id} className="hover:bg-surface-4/30">
                      <td className="px-5 py-3 border-b border-border-light text-[13px] font-medium">{t.term}</td>
                      <td className="px-5 py-3 border-b border-border-light text-[12px] text-fg-3">{t.term_type}</td>
                      <td className="px-5 py-3 border-b border-border-light text-[12px] tabular-nums">{t.priority}</td>
                      <td className="px-5 py-3 border-b border-border-light"><StatusBadge value={t.status} /></td>
                      <td className="px-5 py-3 border-b border-border-light text-[11px] text-fg-4">{formatDate(t.updated_at)}</td>
                    </tr>
                  ))}</tbody>
                </table>
              )}
            </section>

            <section className="card !p-0 overflow-hidden">
              <div className="px-5 py-3 border-b border-border flex items-center justify-between">
                <h2 className="text-[13px] font-semibold">Company Aliases</h2>
                <span className="text-[11px] text-fg-4">{aliases.length} total</span>
              </div>
              {!aliases.length ? <div className="p-5"><Empty title="No aliases yet" message="Aliases map common STT distortions to approved terms." /></div> : (
                <table className="w-full">
                  <thead><tr>{['Heard', 'Correct', 'Safety', 'Status'].map(h => <th key={h} className="text-[10px] font-medium text-fg-4 text-left px-5 py-3 border-b border-border uppercase tracking-wider">{h}</th>)}</tr></thead>
                  <tbody>{aliases.map(a => (
                    <tr key={a.id} className="hover:bg-surface-4/30">
                      <td className="px-5 py-3 border-b border-border-light text-[13px] font-mono">{a.transcript_form}</td>
                      <td className="px-5 py-3 border-b border-border-light text-[13px] font-medium">{a.correct_form}</td>
                      <td className="px-5 py-3 border-b border-border-light text-[11px] text-fg-4">{a.safety_status}</td>
                      <td className="px-5 py-3 border-b border-border-light"><StatusBadge value={a.status} /></td>
                    </tr>
                  ))}</tbody>
                </table>
              )}
            </section>
          </div>

          <aside className="space-y-4">
            <section className="card">
              <div className="flex items-center gap-2 mb-4">
                <BookOpenText size={15} className="text-info" />
                <h2 className="text-[13px] font-semibold">Add Term</h2>
              </div>
              <div className="space-y-2">
                <input className={`${inputClass} w-full`} placeholder="Macobs" value={termForm.term} onChange={e => setTermForm({ ...termForm, term: e.target.value })} />
                <div className="grid grid-cols-2 gap-2">
                  <select className={inputClass} value={termForm.term_type} onChange={e => setTermForm({ ...termForm, term_type: e.target.value })}>
                    {['brand', 'acronym', 'proper_noun', 'code_identifier', 'phrase', 'other'].map(t => <option key={t}>{t}</option>)}
                  </select>
                  <input className={inputClass} placeholder="Priority" value={termForm.priority} onChange={e => setTermForm({ ...termForm, priority: e.target.value })} />
                </div>
                <button className={`${buttonClass} w-full`} disabled={busy} onClick={() => void addTerm()}><Plus size={14} /> Add term</button>
              </div>
            </section>

            <section className="card">
              <h2 className="text-[13px] font-semibold mb-4">Add Alias</h2>
              <div className="space-y-2">
                <input className={`${inputClass} w-full`} placeholder="mecobs" value={aliasForm.transcript_form} onChange={e => setAliasForm({ ...aliasForm, transcript_form: e.target.value })} />
                <input className={`${inputClass} w-full`} placeholder="Macobs" value={aliasForm.correct_form} onChange={e => setAliasForm({ ...aliasForm, correct_form: e.target.value })} />
                <button className={`${buttonClass} w-full`} disabled={busy} onClick={() => void addAlias()}><Plus size={14} /> Add alias</button>
              </div>
            </section>
          </aside>
        </div>
      )}

      {tab === 'suggestions' && (
        <section className="card !p-0 overflow-hidden">
          {!suggestions.length ? <div className="p-5"><Empty title="No suggestions yet" message="Run aggregate after users upload vocabulary summaries." /></div> : (
            <table className="w-full">
              <thead><tr>{['Suggestion', 'Users', 'Confidence', 'Safety', 'Status', 'Actions'].map(h => <th key={h} className="text-[10px] font-medium text-fg-4 text-left px-5 py-3 border-b border-border uppercase tracking-wider">{h}</th>)}</tr></thead>
              <tbody>{suggestions.map(s => (
                <tr key={s.id} className="hover:bg-surface-4/30">
                  <td className="px-5 py-3 border-b border-border-light">
                    <div className="text-[13px] font-medium">{s.kind === 'alias' ? `${s.transcript_form} → ${s.correct_form}` : s.term}</div>
                    <div className="text-[10px] text-fg-4">{s.kind}{s.term_type ? ` · ${s.term_type}` : ''}</div>
                  </td>
                  <td className="px-5 py-3 border-b border-border-light text-[12px] tabular-nums">{s.users_count}</td>
                  <td className="px-5 py-3 border-b border-border-light text-[12px] tabular-nums">{s.confidence.toFixed(2)}</td>
                  <td className="px-5 py-3 border-b border-border-light text-[11px] text-fg-4">{s.safety_status}</td>
                  <td className="px-5 py-3 border-b border-border-light"><StatusBadge value={s.status} /></td>
                  <td className="px-5 py-3 border-b border-border-light">
                    <div className="flex gap-1.5">
                      <button className={ghostButtonClass} disabled={busy || s.status !== 'pending'} onClick={() => void suggestionAction(s.id, 'approve')}><Check size={12} /> Approve</button>
                      <button className={ghostButtonClass} disabled={busy || s.status !== 'pending'} onClick={() => void suggestionAction(s.id, 'reject')}><X size={12} /> Reject</button>
                      <button className={ghostButtonClass} disabled={busy || s.status !== 'pending'} onClick={() => void suggestionAction(s.id, 'block')}><CircleSlash size={12} /> Block</button>
                    </div>
                  </td>
                </tr>
              ))}</tbody>
            </table>
          )}
        </section>
      )}

      {tab === 'releases' && (
        <section className="card !p-0 overflow-hidden">
          {!releases.length ? <div className="p-5"><Empty title="No published bucket yet" message="Approve terms and aliases, then publish a release." /></div> : (
            <table className="w-full">
              <thead><tr>{['Version', 'Hash', 'Notes', 'Created'].map(h => <th key={h} className="text-[10px] font-medium text-fg-4 text-left px-5 py-3 border-b border-border uppercase tracking-wider">{h}</th>)}</tr></thead>
              <tbody>{releases.map(r => (
                <tr key={r.id}>
                  <td className="px-5 py-3 border-b border-border-light text-[13px] font-semibold">v{r.version}</td>
                  <td className="px-5 py-3 border-b border-border-light text-[11px] font-mono text-fg-3 max-w-[360px] truncate">{r.bucket_hash}</td>
                  <td className="px-5 py-3 border-b border-border-light text-[12px] text-fg-3">{r.notes || '--'}</td>
                  <td className="px-5 py-3 border-b border-border-light text-[11px] text-fg-4">{formatDate(r.created_at)}</td>
                </tr>
              ))}</tbody>
            </table>
          )}
        </section>
      )}
    </>
  )
}
