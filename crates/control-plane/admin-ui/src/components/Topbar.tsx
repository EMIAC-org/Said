import { useNavigate } from 'react-router'
import { Search, Bell, Plus, Sun, Moon } from 'lucide-react'
import { useTheme } from '../hooks/useTheme'
import { useAuth } from '../hooks/useAuth'
import { Avatar } from './Avatar'

export function Topbar() {
  const navigate = useNavigate()
  const { theme, toggle } = useTheme()
  const { user } = useAuth()
  const email = user?.account?.email || ''
  const name = email.split('@')[0] || 'Admin'
  const display = name.charAt(0).toUpperCase() + name.slice(1)

  return (
    <div className="flex items-center justify-between px-6 py-4 shrink-0">
      {/* Left — filter pill */}
      <div className="flex items-center gap-3">
        <button className="text-xs font-medium px-4 py-2 rounded-xl border border-border bg-surface text-fg-2 hover:border-fg-5 transition-colors">
          This Month
        </button>
      </div>

      {/* Right */}
      <div className="flex items-center gap-2">
        <div className="flex items-center gap-2 px-3 py-1.5 rounded-xl border border-border bg-surface text-xs text-fg-4 min-w-[180px] cursor-text">
          <Search size={13} className="opacity-40" />
          <span>Search...</span>
          <kbd className="ml-auto text-[9px] bg-floor border border-border px-1.5 py-0.5 rounded text-fg-5 font-mono">/</kbd>
        </div>

        <button onClick={toggle} className="w-8 h-8 rounded-xl flex items-center justify-center text-fg-4 hover:text-fg-3 hover:bg-surface border border-transparent hover:border-border transition-all" title="Toggle theme">
          {theme === 'dark' ? <Sun size={15} /> : <Moon size={15} />}
        </button>

        <button className="w-8 h-8 rounded-xl flex items-center justify-center text-fg-4 hover:text-fg-3 hover:bg-surface border border-transparent hover:border-border transition-all relative">
          <Bell size={15} />
          <span className="absolute top-1.5 right-1.5 w-1.5 h-1.5 rounded-full bg-accent" />
        </button>

        <button
          onClick={() => navigate('/meetings/new')}
          className="inline-flex items-center gap-1.5 text-xs font-semibold px-4 py-2 rounded-xl bg-accent text-accent-fg hover:bg-accent-hover shadow-[0_2px_8px_var(--color-accent-glow)] transition-all ml-1"
        >
          <Plus size={13} strokeWidth={2.5} /> New Meeting
        </button>

        <Avatar name={display} className="ml-1 cursor-pointer" />
      </div>
    </div>
  )
}
