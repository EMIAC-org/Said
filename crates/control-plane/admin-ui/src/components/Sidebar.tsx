import { NavLink } from 'react-router'
import { useAuth } from '../hooks/useAuth'
import { initials } from '../lib/format'

const ICON = {
  overview: (
    <svg className="ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7">
      <rect x="3" y="3" width="7" height="9" rx="1.5" /><rect x="14" y="3" width="7" height="5" rx="1.5" />
      <rect x="14" y="12" width="7" height="9" rx="1.5" /><rect x="3" y="16" width="7" height="5" rx="1.5" />
    </svg>
  ),
  runs: (
    <svg className="ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7">
      <path d="M4 6h16M4 12h16M4 18h10" />
    </svg>
  ),
  meetings: (
    <svg className="ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7">
      <rect x="2" y="6" width="14" height="12" rx="2" /><path d="M16 10l6-3v10l-6-3" />
    </svg>
  ),
  people: (
    <svg className="ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7">
      <circle cx="9" cy="8" r="3.2" /><path d="M3.5 20a5.5 5.5 0 0 1 11 0" />
      <path d="M16 5.2a3.2 3.2 0 0 1 0 6" /><path d="M17.5 20a5.5 5.5 0 0 0-3-4.9" />
    </svg>
  ),
}

function Item({ to, icon, label, end }: { to: string; icon: React.ReactNode; label: string; end?: boolean }) {
  return (
    <NavLink to={to} end={end} className={({ isActive }) => `nav-item${isActive ? ' active' : ''}`}>
      {icon}
      {label}
    </NavLink>
  )
}

export function Sidebar() {
  const { user } = useAuth()
  const email = user?.account?.email || ''
  const name = email.split('@')[0] || 'Admin'
  const display = name.charAt(0).toUpperCase() + name.slice(1)

  return (
    <aside className="sidebar">
      <div className="brand">
        <div className="brand-mark">A</div>
        <div>
          <div className="brand-name">AirNote</div>
          <div className="brand-sub">Enterprise Admin</div>
        </div>
      </div>

      <div className="nav-group">
        <Item to="/" end icon={ICON.overview} label="Overview" />
      </div>

      <div className="nav-group">
        <div className="nav-group-label">Observability</div>
        <Item to="/runs" icon={ICON.runs} label="Runs" />
        <Item to="/meetings" icon={ICON.meetings} label="Meetings" />
      </div>

      <div className="nav-group">
        <div className="nav-group-label">People</div>
        <Item to="/people" icon={ICON.people} label="People" />
      </div>

      <div className="sidebar-foot">
        <div className="user-chip">
          <div className="avatar">{initials(display)}</div>
          <div style={{ minWidth: 0 }}>
            <div style={{ fontSize: 12.5, fontWeight: 500, color: 'var(--ink)' }}>{display}</div>
            <div style={{ fontSize: 11, color: 'var(--muted)', overflow: 'hidden', textOverflow: 'ellipsis' }}>{email}</div>
          </div>
        </div>
      </div>
    </aside>
  )
}
