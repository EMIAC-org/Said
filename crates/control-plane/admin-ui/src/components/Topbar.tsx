import { useLocation } from 'react-router'
import { useTheme } from '../hooks/useTheme'
import { useWindowRange, type Win } from '../lib/window'
import { useSearch } from './Search'

const SECTION: { match: (p: string) => boolean; label: string }[] = [
  { match: p => p === '/', label: 'Overview' },
  { match: p => p.startsWith('/runs'), label: 'Runs' },
  { match: p => p.startsWith('/meetings'), label: 'Meetings' },
  { match: p => p.startsWith('/people'), label: 'People' },
]

const WINDOWS: { w: Win; label: string }[] = [
  { w: 'today', label: 'Today' },
  { w: '7d', label: '7 days' },
  { w: '30d', label: '30 days' },
  { w: 'all', label: 'All' },
]

export function Topbar() {
  const { pathname } = useLocation()
  const { theme, toggle } = useTheme()
  const { win, setWin } = useWindowRange()
  const search = useSearch()
  const section = SECTION.find(s => s.match(pathname))?.label ?? 'Overview'

  return (
    <div className="topbar">
      <div className="crumb"><b>{section}</b></div>
      <div className="spacer" />
      <div className="search" onClick={search.open}>
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8"><circle cx="11" cy="11" r="7" /><path d="M21 21l-4-4" /></svg>
        <span>Search runs, people…</span>
        <span className="kbd mono">⌘K</span>
      </div>
      <div className="seg">
        {WINDOWS.map(x => (
          <button key={x.w} className={win === x.w ? 'active' : ''} onClick={() => setWin(x.w)}>
            {x.label}
          </button>
        ))}
      </div>
      <div className="icon-btn" onClick={toggle} title="Toggle theme">
        {theme === 'dark' ? (
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7">
            <circle cx="12" cy="12" r="4" />
            <path d="M12 2v2M12 20v2M4 12H2M22 12h-2M5 5l1.5 1.5M17.5 17.5 19 19M19 5l-1.5 1.5M6.5 17.5 5 19" />
          </svg>
        ) : (
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7">
            <path d="M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z" />
          </svg>
        )}
      </div>
    </div>
  )
}
