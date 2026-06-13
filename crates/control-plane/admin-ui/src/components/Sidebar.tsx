import { NavLink } from 'react-router'
import { Activity, BarChart3, BookOpenText, Bug, LayoutDashboard, Video, Users, Monitor, Settings, LogOut, LogIn, Gauge } from 'lucide-react'
import { useAuth } from '../hooks/useAuth'
import { Avatar } from './Avatar'

const nav = [
  { to: '/', icon: LayoutDashboard, label: 'Dashboard' },
  { to: '/meetings', icon: Video, label: 'Meetings' },
  { to: '/team', icon: Users, label: 'Team' },
  { to: '/desktop', icon: Monitor, label: 'Desktop' },
  { to: '/runtime', icon: Gauge, label: 'Runtime' },
  { to: '/telemetry', icon: BarChart3, label: 'Telemetry' },
  { to: '/vocabulary', icon: BookOpenText, label: 'Vocabulary' },
  { to: '/bugs', icon: Bug, label: 'Bugs' },
  { to: '/diagnostics', icon: Activity, label: 'Diagnostics' },
  { to: '/settings', icon: Settings, label: 'Settings' },
]

export function Sidebar() {
  const { user, logout, token } = useAuth()
  const email = user?.account?.email || ''
  const name = email.split('@')[0] || 'Admin'
  const display = name.charAt(0).toUpperCase() + name.slice(1)
  const today = new Date()
  const dateStr = today.toLocaleDateString('en-US', { weekday: 'long', month: 'short', day: 'numeric' }).toUpperCase()

  return (
    <aside className="w-[210px] min-w-[210px] bg-sidebar flex flex-col h-screen overflow-y-auto">
      {/* Brand mark */}
      <div className="px-5 pt-5 pb-1 flex items-center gap-2.5">
        <svg viewBox="0 0 24 24" width={22} height={22} className="text-accent shrink-0">
          <rect x="3" y="8.5" width="3" height="7" rx="1.5" fill="currentColor" />
          <rect x="8" y="4.5" width="3" height="15" rx="1.5" fill="currentColor" />
          <rect x="13" y="2.5" width="3" height="19" rx="1.5" fill="currentColor" />
          <rect x="18" y="6.5" width="3" height="11" rx="1.5" fill="currentColor" />
        </svg>
        <span className="text-[13px] font-semibold tracking-tight text-fg">AirNote Enterprise</span>
      </div>

      {/* User greeting */}
      <div className="px-5 pt-4 pb-5">
        <div className="text-[9px] font-medium text-fg-4 tracking-[0.06em] mb-2.5">{dateStr}</div>
        <div className="flex items-center gap-2.5">
          <Avatar name={display} size="lg" />
          <div>
            <div className="text-[10px] text-fg-4">Welcome back,</div>
            <div className="text-[13px] font-semibold leading-tight text-fg">{display}!</div>
          </div>
        </div>
      </div>

      {/* Nav */}
      <nav className="flex flex-col gap-px px-3 flex-1">
        {nav.map(n => (
          <NavLink
            key={n.to}
            to={n.to}
            end={n.to === '/'}
            className={({ isActive }) =>
              `flex items-center gap-2.5 px-2.5 h-[30px] rounded-md text-[12.5px] transition-colors ${
                isActive
                  ? 'text-fg font-medium bg-surface-4/60 shadow-[inset_0_0_0_1px_hsla(0,0%,100%,0.06)]'
                  : 'text-fg-3 font-normal hover:text-fg hover:bg-[rgba(255,255,255,0.03)]'
              }`
            }
          >
            <n.icon size={15} strokeWidth={1.6} />
            {n.label}
          </NavLink>
        ))}
      </nav>

      {/* Bottom */}
      <div className="px-3 pb-4 mt-auto">
        {token ? (
          <button
            onClick={logout}
            className="flex items-center gap-2 w-full px-2.5 py-[6px] text-[11px] text-fg-4 hover:text-fg-3 rounded-md hover:bg-[rgba(255,255,255,0.03)] transition-colors"
          >
            <LogOut size={13} strokeWidth={1.6} /> Sign out
          </button>
        ) : (
          <NavLink
            to="/login"
            className="flex items-center gap-2 w-full px-2.5 py-[6px] text-[11px] text-accent hover:text-accent-hover rounded-md hover:bg-[rgba(255,255,255,0.03)] transition-colors"
          >
            <LogIn size={13} strokeWidth={1.6} /> Sign in
          </NavLink>
        )}
      </div>
    </aside>
  )
}
