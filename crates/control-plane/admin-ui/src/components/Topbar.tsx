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
        <button className="text-xs font-medium px-4 py-2 rounded-lg border border-border bg-transparent text-fg-3 hover:text-fg hover:border-fg-5 transition-colors">
          This Month
        </button>
      </div>

      {/* Right */}
      <div className="flex items-center gap-2">
        <div className="flex items-center gap-2 px-3 py-1.5 rounded-lg border border-border bg-[hsla(0,0%,0%,0.25)] text-xs text-fg-4 min-w-[180px] cursor-text focus-within:border-accent/45 focus-within:shadow-[0_0_0_3px_hsla(226,80%,78%,0.10)] transition-all">
          <Search size={13} className="opacity-40" />
          <span>Search...</span>
          <kbd className="ml-auto text-[9px] bg-surface-4 border border-border px-1.5 py-0.5 rounded text-fg-4 font-mono">/</kbd>
        </div>

        <button onClick={toggle} className="w-8 h-8 rounded-lg flex items-center justify-center text-fg-4 hover:text-fg-3 hover:bg-surface-4/50 border border-transparent hover:border-border transition-all" title="Toggle theme">
          {theme === 'dark' ? <Sun size={15} /> : <Moon size={15} />}
        </button>

        <button className="w-8 h-8 rounded-lg flex items-center justify-center text-fg-4 hover:text-fg-3 hover:bg-surface-4/50 border border-transparent hover:border-border transition-all relative">
          <Bell size={15} />
          <span className="absolute top-1.5 right-1.5 w-1.5 h-1.5 rounded-full bg-accent" />
        </button>

        <button
          onClick={() => navigate('/meetings/new')}
          className="inline-flex items-center gap-1.5 text-[13px] font-semibold px-4 h-9 rounded-lg bg-[hsl(0_0%_98%)] text-[hsl(240_8%_8%)] hover:opacity-90 hover:-translate-y-px active:translate-y-0 transition-all ml-1"
        >
          <Plus size={13} strokeWidth={2.5} /> New Meeting
        </button>

        <Avatar name={display} className="ml-1 cursor-pointer" />
      </div>
    </div>
  )
}
