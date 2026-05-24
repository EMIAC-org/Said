import { useState, type FormEvent } from 'react'
import { Navigate } from 'react-router'
import { useAuth } from '../hooks/useAuth'

export function LoginPage() {
  const { token, login } = useAuth()
  const [signup, setSignup] = useState(false)
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')
  const [error, setError] = useState('')
  const [loading, setLoading] = useState(false)

  if (token) return <Navigate to="/" replace />

  const submit = async (e: FormEvent) => {
    e.preventDefault()
    if (!email || password.length < 8) { setError('Email required, password min 8 characters.'); return }
    setLoading(true); setError('')
    try { await login(email, password, signup) }
    catch (err) { setError((err as Error).message); setLoading(false) }
  }

  return (
    <div className="min-h-screen flex items-center justify-center bg-floor relative">
      {/* Hero glow */}
      <div className="hero-glow" />

      <div className="glass-strong w-[400px] !rounded-[18px] p-10 relative z-10">
        {/* Brand mark */}
        <div className="flex flex-col items-center gap-2.5 mb-8">
          <svg width={36} height={36} viewBox="0 0 24 24" className="text-accent">
            <rect x="3" y="8.5" width="3" height="7" rx="1.5" fill="currentColor" />
            <rect x="8" y="4.5" width="3" height="15" rx="1.5" fill="currentColor" />
            <rect x="13" y="2.5" width="3" height="19" rx="1.5" fill="currentColor" />
            <rect x="18" y="6.5" width="3" height="11" rx="1.5" fill="currentColor" />
          </svg>
          <span className="text-[16px] font-bold tracking-tight">AirNote Enterprise</span>
        </div>

        <h1 className="text-lg font-semibold text-center mb-1">Welcome back</h1>
        <p className="text-[13px] text-fg-3 text-center mb-7">Sign in to your admin dashboard</p>

        {error && <div className="bg-live-bg border border-live/20 rounded-lg px-3.5 py-2.5 text-xs text-live mb-5">{error}</div>}

        <form onSubmit={submit}>
          <div className="mb-5">
            <label className="block text-[11px] font-semibold text-fg-3 mb-1.5 uppercase tracking-[0.08em]">Email</label>
            <input type="email" className="w-full px-3.5 py-2.5 text-[13px] bg-[hsla(0,0%,0%,0.25)] border border-border rounded-lg outline-none focus:bg-[hsla(0,0%,0%,0.35)] focus:border-[hsla(226,80%,78%,0.45)] focus:shadow-[0_0_0_3px_hsla(226,80%,78%,0.10)] transition placeholder:text-fg-4 text-fg" placeholder="you@company.com" value={email} onChange={e => setEmail(e.target.value)} required autoComplete="email" />
          </div>
          <div className="mb-6">
            <label className="block text-[11px] font-semibold text-fg-3 mb-1.5 uppercase tracking-[0.08em]">Password</label>
            <input type="password" className="w-full px-3.5 py-2.5 text-[13px] bg-[hsla(0,0%,0%,0.25)] border border-border rounded-lg outline-none focus:bg-[hsla(0,0%,0%,0.35)] focus:border-[hsla(226,80%,78%,0.45)] focus:shadow-[0_0_0_3px_hsla(226,80%,78%,0.10)] transition placeholder:text-fg-4 text-fg" placeholder="Enter your password" value={password} onChange={e => setPassword(e.target.value)} required autoComplete="current-password" />
          </div>
          <button type="submit" disabled={loading} className="w-full flex items-center justify-center gap-2 text-[13px] font-semibold px-4 h-9 rounded-lg bg-[hsl(0_0%_98%)] text-[hsl(240_8%_8%)] hover:opacity-92 hover:-translate-y-px active:translate-y-0 disabled:opacity-35 disabled:cursor-not-allowed transition-all">
            {loading ? <div className="spinner" style={{ width: 14, height: 14, borderWidth: 2 }} /> : (signup ? 'Create Account' : 'Sign In')}
          </button>
        </form>

        <div className="text-center mt-6 text-xs text-fg-4">
          {signup ? 'Already have an account?' : "Don't have an account?"}{' '}
          <a className="text-accent font-medium cursor-pointer hover:underline" onClick={() => { setSignup(!signup); setError('') }}>
            {signup ? 'Sign In' : 'Sign Up'}
          </a>
        </div>
      </div>
    </div>
  )
}
