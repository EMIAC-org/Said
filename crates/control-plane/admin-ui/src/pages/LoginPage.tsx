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
    <div className="auth-wrap">
      <div className="auth-card card card-pad">
        <div className="brand">
          <div className="brand-mark">A</div>
          <div>
            <div className="brand-name">AirNote</div>
            <div className="brand-sub">Enterprise Admin</div>
          </div>
        </div>

        <h1 className="auth-title">{signup ? 'Create your account' : 'Welcome back'}</h1>
        <p className="auth-sub">Sign in to the admin console</p>

        {error && <div className="errbox" style={{ marginBottom: 16 }}><p>{error}</p></div>}

        <form onSubmit={submit}>
          <div className="field">
            <label>Email</label>
            <input className="input" type="email" placeholder="you@company.com" value={email} onChange={e => setEmail(e.target.value)} required autoComplete="email" />
          </div>
          <div className="field">
            <label>Password</label>
            <input className="input" type="password" placeholder="Enter your password" value={password} onChange={e => setPassword(e.target.value)} required autoComplete="current-password" />
          </div>
          <button type="submit" disabled={loading} className="btn primary block" style={{ marginTop: 6 }}>
            {loading ? <span className="spinner" style={{ width: 14, height: 14 }} /> : signup ? 'Create account' : 'Sign in'}
          </button>
        </form>

        <div className="auth-alt">
          {signup ? 'Already have an account?' : "Don't have an account?"}{' '}
          <button onClick={() => { setSignup(!signup); setError('') }}>{signup ? 'Sign in' : 'Sign up'}</button>
        </div>
      </div>
    </div>
  )
}
