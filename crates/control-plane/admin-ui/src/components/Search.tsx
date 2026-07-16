import { createContext, useContext, useState, useCallback, useEffect, useRef, type ReactNode } from 'react'
import { useNavigate } from 'react-router'
import { apiJson } from '../api'
import { useAuth } from '../hooks/useAuth'
import { Avatar } from './ui'
import type { PeopleResponse, PersonRow } from '../lib/adminTypes'

interface SearchCtx { open: () => void }
const Ctx = createContext<SearchCtx>(null!)
export function useSearch() { return useContext(Ctx) }

const PAGES = [
  { label: 'Overview', to: '/', kw: 'overview home dashboard' },
  { label: 'Runs', to: '/runs', kw: 'runs dictation logs trace cost' },
  { label: 'People', to: '/people', kw: 'people users team members' },
  { label: 'Meetings', to: '/meetings', kw: 'meetings deepseek cost' },
]

const PageIcon = (
  <svg className="ic" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7"><rect x="3" y="4" width="18" height="16" rx="2" /><path d="M3 9h18" /></svg>
)

function displayName(p: PersonRow) { return p.lark_name || p.email?.split('@')[0] || 'Unknown' }

export function SearchProvider({ children }: { children: ReactNode }) {
  const navigate = useNavigate()
  const { org } = useAuth()
  const [open, setOpen] = useState(false)
  const [query, setQuery] = useState('')
  const [people, setPeople] = useState<PersonRow[]>([])
  const [active, setActive] = useState(0)
  const inputRef = useRef<HTMLInputElement>(null)
  const orgId = org?.org?.id

  const openPalette = useCallback(() => setOpen(true), [])
  const close = useCallback(() => { setOpen(false); setQuery('') }, [])

  // Global ⌘K / Ctrl+K
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') {
        e.preventDefault()
        setOpen(o => !o)
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  // Lazy-load people the first time it opens
  useEffect(() => {
    if (open && orgId && people.length === 0) {
      apiJson<PeopleResponse>(`/v1/orgs/${orgId}/telemetry/users?days=all&limit=500`)
        .then(r => setPeople(r.users || []))
        .catch(() => {})
    }
    if (open) { setActive(0); setTimeout(() => inputRef.current?.focus(), 0) }
  }, [open, orgId, people.length])

  const q = query.trim().toLowerCase()
  const pageHits = PAGES.filter(p => !q || p.label.toLowerCase().includes(q) || p.kw.includes(q))
  const peopleHits = q
    ? people.filter(p => displayName(p).toLowerCase().includes(q) || (p.email || '').toLowerCase().includes(q)).slice(0, 8)
    : []

  const flat: { kind: 'page' | 'person'; to: string; person?: PersonRow; label: string }[] = [
    ...pageHits.map(p => ({ kind: 'page' as const, to: p.to, label: p.label })),
    ...peopleHits.map(p => ({ kind: 'person' as const, to: `/people/${p.account_id}`, person: p, label: displayName(p) })),
  ]

  useEffect(() => { setActive(a => Math.min(a, Math.max(0, flat.length - 1))) }, [flat.length])

  const select = useCallback((i: number) => {
    const item = flat[i]
    if (!item) return
    navigate(item.to)
    close()
  }, [flat, navigate, close])

  const onInputKey = (e: React.KeyboardEvent) => {
    if (e.key === 'ArrowDown') { e.preventDefault(); setActive(a => Math.min(a + 1, flat.length - 1)) }
    else if (e.key === 'ArrowUp') { e.preventDefault(); setActive(a => Math.max(a - 1, 0)) }
    else if (e.key === 'Enter') { e.preventDefault(); select(active) }
    else if (e.key === 'Escape') { e.preventDefault(); close() }
  }

  return (
    <Ctx.Provider value={{ open: openPalette }}>
      {children}
      {open && (
        <div className="cmdk-scrim" onClick={close}>
          <div className="cmdk" onClick={e => e.stopPropagation()}>
            <div className="cmdk-inputrow">
              <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="var(--muted)" strokeWidth="1.8"><circle cx="11" cy="11" r="7" /><path d="M21 21l-4-4" /></svg>
              <input
                ref={inputRef}
                className="cmdk-input"
                placeholder="Search runs, people…"
                value={query}
                onChange={e => setQuery(e.target.value)}
                onKeyDown={onInputKey}
              />
            </div>

            <div className="cmdk-list">
              {flat.length === 0 ? (
                <div className="cmdk-empty">No matches for “{query}”.</div>
              ) : (
                <>
                  {pageHits.length > 0 && <div className="cmdk-group">Go to</div>}
                  {pageHits.map((p, i) => {
                    const idx = i
                    return (
                      <div key={p.to} className={`cmdk-item${active === idx ? ' active' : ''}`}
                        onMouseEnter={() => setActive(idx)} onClick={() => select(idx)}>
                        {PageIcon}
                        <span className="t">{p.label}</span>
                        <span className="go">↵</span>
                      </div>
                    )
                  })}
                  {peopleHits.length > 0 && <div className="cmdk-group">People</div>}
                  {peopleHits.map((p, i) => {
                    const idx = pageHits.length + i
                    return (
                      <div key={p.account_id} className={`cmdk-item${active === idx ? ' active' : ''}`}
                        onMouseEnter={() => setActive(idx)} onClick={() => select(idx)}>
                        <Avatar name={displayName(p)} size={22} />
                        <span className="t">{displayName(p)}</span>
                        <span className="s">{p.email}</span>
                        <span className="go">↵</span>
                      </div>
                    )
                  })}
                </>
              )}
            </div>

            <div className="cmdk-foot">
              <span><span className="kbd">↑↓</span> navigate</span>
              <span><span className="kbd">↵</span> open</span>
              <span><span className="kbd">esc</span> close</span>
            </div>
          </div>
        </div>
      )}
    </Ctx.Provider>
  )
}
