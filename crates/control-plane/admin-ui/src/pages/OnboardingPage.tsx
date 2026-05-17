import { useEffect, useMemo, useState, type FormEvent } from 'react'
import { Navigate, useNavigate } from 'react-router'
import { ArrowRight, Building2, Check, ExternalLink, LogOut } from 'lucide-react'
import { apiJson } from '../api'
import { useAuth } from '../hooks/useAuth'
import { Loading } from '../components/States'

function slugify(value: string) {
  return value
    .toLowerCase()
    .trim()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 48)
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
    if (!name && suggestedName) {
      setName(suggestedName)
      setSlug(slugify(suggestedName))
    }
  }, [name, suggestedName])

  if (loading) return <div className="h-screen flex items-center justify-center bg-floor"><Loading /></div>
  if (!token) return <Navigate to="/login" replace />

  const createOrg = async (e: FormEvent) => {
    e.preventDefault()
    const cleanName = name.trim()
    const cleanSlug = slugify(slug || cleanName)
    if (!cleanName || !cleanSlug) {
      setError('Organization name is required.')
      return
    }

    setBusy('org')
    setError('')
    try {
      await apiJson('/v1/orgs', {
        method: 'POST',
        body: JSON.stringify({
          name: cleanName,
          slug: cleanSlug,
          meeting_creator_roles: ['COMPANY_ADMIN', 'MANAGER'],
        }),
      })
      await refreshOrg()
    } catch (e) {
      setError((e as Error).message)
    }
    setBusy('')
  }

  const connectLark = async () => {
    setBusy('lark')
    setError('')
    try {
      const data = await apiJson<{ url: string }>('/v1/auth/lark/start')
      window.location.href = data.url
    } catch (e) {
      setError((e as Error).message)
      setBusy('')
    }
  }

  const hasOrg = !!org?.org

  return (
    <div className="min-h-screen bg-floor text-fg flex items-center justify-center px-5 py-8 relative">
      <div className="hero-glow" />

      <div className="w-full max-w-[920px] grid grid-cols-[1.05fr_0.95fr] gap-5 relative z-10">
        <section className="card !p-8 flex flex-col justify-between min-h-[520px]">
          <div>
            <div className="w-11 h-11 rounded-xl bg-surface-4 text-accent flex items-center justify-center mb-6">
              <Building2 size={20} />
            </div>
            <h1 className="text-[26px] font-semibold tracking-tight leading-tight">Set up your workspace</h1>
            <p className="text-[13px] text-fg-3 leading-relaxed mt-3 max-w-[460px]">
              Create the Said organization that will own meetings, members, Lark sync, summaries, and task handoff.
            </p>
          </div>

          <div className="space-y-3 mt-8">
            {[
              'Create your Said organization',
              'Connect Lark with the server app credentials',
              'Invite members and start meetings',
            ].map((item, idx) => (
              <div key={item} className="flex items-center gap-3">
                <div className={`w-7 h-7 rounded-full flex items-center justify-center text-[11px] font-semibold ${idx === 0 || hasOrg ? 'bg-[hsl(0_0%_98%)] text-[hsl(240_8%_8%)]' : 'bg-surface-4 border border-border text-fg-4'}`}>
                  {idx === 0 && hasOrg ? <Check size={14} /> : idx + 1}
                </div>
                <span className="text-[13px] text-fg-2">{item}</span>
              </div>
            ))}
          </div>
        </section>

        <section className="card !p-8">
          <div className="flex items-start justify-between mb-6">
            <div>
              <h2 className="text-[17px] font-semibold tracking-tight">{hasOrg ? 'Organization ready' : 'Create organization'}</h2>
              <p className="text-[12px] text-fg-4 mt-1">{user?.account?.email}</p>
            </div>
            <button onClick={logout} className="w-8 h-8 rounded-lg flex items-center justify-center text-fg-4 hover:text-fg-2 hover:bg-surface-4/30 transition-colors" title="Sign out">
              <LogOut size={15} />
            </button>
          </div>

          {error && <div className="bg-live-bg border border-live/20 rounded-lg px-3.5 py-2.5 text-xs text-live mb-5">{error}</div>}

          {!hasOrg ? (
            <form onSubmit={createOrg} className="space-y-5">
              <div>
                <label className="block text-[12px] font-medium text-fg-3 mb-1.5">Organization name</label>
                <input
                  value={name}
                  onChange={e => {
                    setName(e.target.value)
                    if (!slugEdited) setSlug(slugify(e.target.value))
                  }}
                  className="w-full px-3.5 py-2.5 text-[13px] bg-[hsla(0,0%,0%,0.25)] border border-border rounded-lg outline-none focus:border-[hsla(226,80%,78%,0.45)] focus:shadow-[0_0_0_3px_hsla(226,80%,78%,0.10)] transition placeholder:text-fg-4 text-fg"
                  placeholder="Acme"
                  autoFocus
                />
              </div>
              <div>
                <label className="block text-[12px] font-medium text-fg-3 mb-1.5">Workspace slug</label>
                <input
                  value={slug}
                  onChange={e => { setSlugEdited(true); setSlug(slugify(e.target.value)) }}
                  className="w-full px-3.5 py-2.5 text-[13px] font-mono bg-[hsla(0,0%,0%,0.25)] border border-border rounded-lg outline-none focus:border-[hsla(226,80%,78%,0.45)] focus:shadow-[0_0_0_3px_hsla(226,80%,78%,0.10)] transition placeholder:text-fg-4 text-fg"
                  placeholder="acme"
                />
              </div>
              <button type="submit" disabled={busy === 'org'} className="w-full inline-flex items-center justify-center gap-2 text-[13px] font-semibold px-4 h-10 rounded-lg bg-[hsl(0_0%_98%)] text-[hsl(240_8%_8%)] hover:opacity-90 disabled:opacity-35 transition-all">
                {busy === 'org' ? <div className="spinner" style={{ width: 14, height: 14, borderWidth: 2 }} /> : <ArrowRight size={14} />}
                Create organization
              </button>
            </form>
          ) : (
            <div className="space-y-4">
              <div className="rounded-xl border border-ok/25 bg-ok-bg p-4">
                <div className="flex items-center gap-2 text-[13px] font-semibold text-ok mb-1">
                  <Check size={15} /> {org.org.name}
                </div>
                <p className="text-[11px] text-fg-3">Your organization exists. Next, authorize Lark for workspace identity and task sync.</p>
              </div>
              <button onClick={connectLark} disabled={busy === 'lark'} className="w-full inline-flex items-center justify-center gap-2 text-[13px] font-semibold px-4 h-10 rounded-lg bg-[hsl(0_0%_98%)] text-[hsl(240_8%_8%)] hover:opacity-90 disabled:opacity-35 transition-all">
                {busy === 'lark' ? <div className="spinner" style={{ width: 14, height: 14, borderWidth: 2 }} /> : <ExternalLink size={14} />}
                Connect Lark
              </button>
              <button onClick={() => navigate('/')} className="w-full text-[12px] font-medium px-4 h-10 rounded-lg border border-border text-fg-3 hover:text-fg hover:border-fg-5 transition-colors">
                Continue to dashboard
              </button>
            </div>
          )}

          <p className="text-[10px] text-fg-5 mt-6 leading-relaxed">
            The server already has the Lark app ID and secret. This step only creates your Said organization and authorizes your Lark workspace/user.
          </p>
        </section>
      </div>
    </div>
  )
}
