import { useEffect, useMemo, useState, type FormEvent } from 'react'
import { Navigate, useNavigate } from 'react-router'
import { apiJson } from '../api'
import { useAuth } from '../hooks/useAuth'
import { Loading } from '../components/ui'

function slugify(value: string) {
  return value.toLowerCase().trim().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 48)
}

function orgNameFromEmail(email?: string) {
  const domain = email?.split('@')[1]?.split('.')[0]
  if (!domain) return ''
  return domain.charAt(0).toUpperCase() + domain.slice(1)
}

export function OnboardingPage() {
  const navigate = useNavigate()
  const { token, loading, user, org, refreshOrg, logout } = useAuth()
  const suggestedName = useMemo(() => orgNameFromEmail(user?.account?.email), [user?.account?.email])
  const [name, setName] = useState(suggestedName)
  const [slug, setSlug] = useState(slugify(suggestedName))
  const [slugEdited, setSlugEdited] = useState(false)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState('')

  useEffect(() => {
    if (!name && suggestedName) { setName(suggestedName); setSlug(slugify(suggestedName)) }
  }, [name, suggestedName])

  if (loading) return <div className="auth-wrap"><Loading /></div>
  if (!token) return <Navigate to="/login" replace />

  const createOrg = async (e: FormEvent) => {
    e.preventDefault()
    const cleanName = name.trim()
    const cleanSlug = slugify(slug || cleanName)
    if (!cleanName || !cleanSlug) { setError('Organization name is required.'); return }
    setBusy('org'); setError('')
    try {
      await apiJson('/v1/orgs', {
        method: 'POST',
        body: JSON.stringify({ name: cleanName, slug: cleanSlug, meeting_creator_roles: ['COMPANY_ADMIN', 'MANAGER'] }),
      })
      await refreshOrg()
    } catch (e) { setError((e as Error).message) }
    setBusy('')
  }

  const connectLark = async () => {
    setBusy('lark'); setError('')
    try {
      const data = await apiJson<{ url: string }>('/v1/auth/lark/start')
      window.location.href = data.url
    } catch (e) { setError((e as Error).message); setBusy('') }
  }

  const hasOrg = !!org?.org

  return (
    <div className="auth-wrap">
      <div className="auth-card card card-pad">
        <div className="brand">
          <div className="brand-mark">A</div>
          <div>
            <div className="brand-name">AirNote</div>
            <div className="brand-sub">{user?.account?.email}</div>
          </div>
        </div>

        <h1 className="auth-title">{hasOrg ? 'Organization ready' : 'Set up your workspace'}</h1>
        <p className="auth-sub">Create the organization that owns meetings, members, and sync.</p>

        {error && <div className="errbox" style={{ marginBottom: 16 }}><p>{error}</p></div>}

        {!hasOrg ? (
          <form onSubmit={createOrg}>
            <div className="field">
              <label>Organization name</label>
              <input className="input" value={name} placeholder="Acme" autoFocus
                onChange={e => { setName(e.target.value); if (!slugEdited) setSlug(slugify(e.target.value)) }} />
            </div>
            <div className="field">
              <label>Workspace slug</label>
              <input className="input mono" value={slug} placeholder="acme"
                onChange={e => { setSlugEdited(true); setSlug(slugify(e.target.value)) }} />
            </div>
            <button type="submit" disabled={busy === 'org'} className="btn primary block" style={{ marginTop: 6 }}>
              {busy === 'org' ? <span className="spinner" style={{ width: 14, height: 14 }} /> : 'Create organization'}
            </button>
          </form>
        ) : (
          <>
            <div className="hint" style={{ marginBottom: 14 }}>
              <b>{org.org.name}</b> is ready. Next, authorize Lark for workspace identity and task sync.
            </div>
            <button onClick={connectLark} disabled={busy === 'lark'} className="btn primary block" style={{ marginBottom: 10 }}>
              {busy === 'lark' ? <span className="spinner" style={{ width: 14, height: 14 }} /> : 'Connect Lark'}
            </button>
            <button onClick={() => navigate('/')} className="btn block">Continue to console</button>
          </>
        )}

        <div className="auth-alt">
          <button onClick={logout}>Sign out</button>
        </div>
      </div>
    </div>
  )
}
