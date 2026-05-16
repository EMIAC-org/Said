import { useState, useEffect } from 'react'
import { Sun, Moon } from 'lucide-react'
import { apiJson, api } from '../api'
import { useAuth } from '../hooks/useAuth'
import { useTheme } from '../hooks/useTheme'
import { RolePill } from '../components/StatusPill'
import { OpenAILogo, LarkLogo, DeepgramLogo, GroqLogo, SlackLogo, ZoomLogo, GoogleMeetLogo, TeamsLogo, NotionLogo, LinearLogo } from '../components/BrandLogos'

interface OpenAIStatus {
  connected: boolean
  plan_type?: string
  label?: string
  connected_at?: string
}

export function SettingsPage() {
  const { org, refreshOrg } = useAuth()
  const { theme, toggle } = useTheme()
  const [oai, setOai] = useState<OpenAIStatus>({ connected: false })
  const [connectFlow, setConnectFlow] = useState<{ auth_url: string; code_verifier: string } | null>(null)
  const [code, setCode] = useState('')
  const [plan, setPlan] = useState('')
  const [actionLoading, setActionLoading] = useState('')

  useEffect(() => {
    apiJson<OpenAIStatus>('/v1/openai/status').then(setOai).catch(() => {})
  }, [])

  const o = org?.org

  const startConnect = async () => {
    setActionLoading('connect')
    try {
      const d = await apiJson<{ auth_url: string; code_verifier: string }>('/v1/openai/connect', { method: 'POST' })
      setConnectFlow(d)
      window.open(d.auth_url, '_blank')
    } catch (e) { alert('Failed: ' + (e as Error).message) }
    setActionLoading('')
  }

  const completeConnect = async () => {
    if (!code.trim() || !connectFlow) { alert('Paste the callback URL.'); return }
    let extracted = code.trim()
    try { const u = new URL(extracted); const c = u.searchParams.get('code'); if (c) extracted = c } catch {}
    setActionLoading('complete')
    try {
      await apiJson('/v1/openai/complete', { method: 'POST', body: JSON.stringify({ code: extracted, code_verifier: connectFlow.code_verifier, plan_type: plan || null }) })
      refreshOrg()
      setOai({ connected: true, plan_type: plan })
      setConnectFlow(null)
    } catch (e) { alert('Failed: ' + (e as Error).message) }
    setActionLoading('')
  }

  const disconnect = async () => {
    if (!confirm('Disconnect OpenAI?')) return
    setActionLoading('disconnect')
    try { await api('/v1/openai/disconnect', { method: 'DELETE' }); refreshOrg(); setOai({ connected: false }) }
    catch (e) { alert('Failed: ' + (e as Error).message) }
    setActionLoading('')
  }

  return (
    <>
      <h1 className="text-xl font-semibold tracking-tight">Settings</h1>
      <p className="text-[12px] text-fg-4 mt-0.5 mb-6">Manage your organization</p>

      <div className="grid grid-cols-[2fr_1fr] gap-4">
        {/* Left column */}
        <div className="space-y-4">
          {/* Integrations */}
          <div className="card">
            <div className="text-[12px] font-semibold mb-4">Integrations</div>

            {/* OpenAI */}
            <div className={`border rounded-2xl p-4 mb-3 ${oai.connected ? 'border-ok/30' : 'border-border'}`}>
              <div className="flex items-center gap-3 mb-3">
                <div className="w-10 h-10 rounded-xl bg-[#0a0a0a] dark:bg-[#f5f5f4] flex items-center justify-center">
                  <OpenAILogo size={22} className="text-white dark:text-[#0a0a0a]" />
                </div>
                <div className="flex-1">
                  <div className="text-[13px] font-medium">OpenAI</div>
                  <div className="text-[10px] text-fg-4">AI meeting summaries, tasks & decisions</div>
                </div>
                {oai.connected
                  ? <span className="text-[10px] font-semibold px-2.5 py-1 rounded-full bg-ok-bg text-ok">Connected</span>
                  : <span className="text-[10px] font-semibold px-2.5 py-1 rounded-full bg-accent-light text-fg-3">Not connected</span>
                }
              </div>
              {oai.connected ? (
                <div className="flex items-center justify-between pt-3 border-t border-border-light">
                  <div className="text-[11px] text-fg-4">{oai.plan_type ? `Plan: ${oai.plan_type}` : 'Connected'}{oai.connected_at ? ` · Since ${oai.connected_at}` : ''}</div>
                  <button onClick={disconnect} disabled={actionLoading === 'disconnect'} className="text-[11px] font-medium px-3 py-1.5 rounded-lg text-live border border-live/30 hover:bg-live-bg disabled:opacity-40 transition-colors">
                    {actionLoading === 'disconnect' ? <div className="spinner" style={{ width: 14, height: 14, borderWidth: 2 }} /> : 'Disconnect'}
                  </button>
                </div>
              ) : (
                <>
                  {!connectFlow ? (
                    <button onClick={startConnect} disabled={actionLoading === 'connect'} className="inline-flex items-center gap-1.5 text-[11px] font-semibold px-4 py-2 rounded-xl bg-accent text-accent-fg hover:bg-accent-hover shadow-[0_2px_6px_var(--color-accent-glow)] disabled:opacity-40 transition-all mt-1">
                      {actionLoading === 'connect' && <div className="spinner" style={{ width: 14, height: 14, borderWidth: 2 }} />}
                      Connect Account
                    </button>
                  ) : (
                    <div className="border border-border rounded-xl p-4 mt-3 space-y-3">
                      <div>
                        <label className="block text-[11px] font-medium mb-1.5">Authorization URL</label>
                        <input type="text" className="w-full px-3 py-2 text-[10px] font-mono bg-floor border border-border rounded-lg outline-none" value={connectFlow.auth_url} readOnly onClick={e => (e.target as HTMLInputElement).select()} />
                        <p className="text-[10px] text-fg-5 mt-1">Open this URL in your browser.</p>
                      </div>
                      <div>
                        <label className="block text-[11px] font-medium mb-1.5">Callback URL</label>
                        <input type="text" className="w-full px-3 py-2 text-[12px] bg-floor border border-border rounded-lg outline-none focus:border-accent transition placeholder:text-fg-5" placeholder="Paste the full callback URL" value={code} onChange={e => setCode(e.target.value)} />
                      </div>
                      <div>
                        <label className="block text-[11px] font-medium mb-1.5">Plan Type</label>
                        <select className="w-full px-3 py-2 text-[12px] bg-floor border border-border rounded-lg outline-none appearance-none" value={plan} onChange={e => setPlan(e.target.value)}>
                          <option value="">Select plan...</option>
                          {['free', 'plus', 'pro', 'team', 'enterprise'].map(p => <option key={p} value={p}>{p.charAt(0).toUpperCase() + p.slice(1)}</option>)}
                        </select>
                      </div>
                      <button onClick={completeConnect} disabled={actionLoading === 'complete'} className="inline-flex items-center gap-1.5 text-[11px] font-semibold px-4 py-2 rounded-xl bg-accent text-accent-fg hover:bg-accent-hover disabled:opacity-40 transition-all">
                        {actionLoading === 'complete' && <div className="spinner" style={{ width: 14, height: 14, borderWidth: 2 }} />}
                        {actionLoading === 'complete' ? 'Completing...' : 'Complete Connection'}
                      </button>
                    </div>
                  )}
                </>
              )}
            </div>

            {/* Active integrations */}
            {[
              { logo: <LarkLogo size={22} />, name: 'Lark / Feishu', desc: 'OAuth login, task sync, docs', bg: 'bg-[#e8f4ff] dark:bg-[#1a2a3d]' },
              { logo: <DeepgramLogo size={20} />, name: 'Deepgram', desc: 'Real-time speech-to-text', bg: 'bg-[#e8fff0] dark:bg-[#0d2a1a]' },
              { logo: <GroqLogo size={20} />, name: 'Groq', desc: 'Voice polishing & classification', bg: 'bg-[#fff0ed] dark:bg-[#2a1510]' },
            ].map(i => (
              <div key={i.name} className="border border-ok/20 rounded-2xl p-4 mb-3 flex items-center gap-3">
                <div className={`w-10 h-10 rounded-xl ${i.bg} flex items-center justify-center`}>{i.logo}</div>
                <div className="flex-1">
                  <div className="text-[13px] font-medium">{i.name}</div>
                  <div className="text-[10px] text-fg-4">{i.desc}</div>
                </div>
                <span className="text-[10px] font-semibold px-2.5 py-1 rounded-full bg-ok-bg text-ok">Active</span>
              </div>
            ))}
          </div>

          {/* Coming soon */}
          <div className="card">
            <div className="text-[10px] font-semibold text-fg-4 uppercase tracking-wider mb-3">Coming Soon</div>
            <div className="grid grid-cols-2 gap-2.5">
              {[
                { logo: <SlackLogo size={20} />, name: 'Slack', desc: 'Channel notifications', bg: 'bg-[#f8f0ff] dark:bg-[#1f1525]' },
                { logo: <ZoomLogo size={20} />, name: 'Zoom', desc: 'Meeting import & sync', bg: 'bg-[#eef6ff] dark:bg-[#12202d]' },
                { logo: <GoogleMeetLogo size={20} />, name: 'Google Meet', desc: 'Calendar sync', bg: 'bg-[#edfaf7] dark:bg-[#0d2a23]' },
                { logo: <TeamsLogo size={20} />, name: 'MS Teams', desc: 'Teams integration', bg: 'bg-[#eef0ff] dark:bg-[#151729]' },
                { logo: <NotionLogo size={20} />, name: 'Notion', desc: 'Export summaries', bg: 'bg-floor' },
                { logo: <LinearLogo size={20} />, name: 'Linear', desc: 'Auto-create issues', bg: 'bg-[#f0f0ff] dark:bg-[#18182a]' },
              ].map(i => (
                <div key={i.name} className="border border-border rounded-xl p-3.5 flex items-center gap-3 opacity-60 hover:opacity-90 transition-opacity cursor-default">
                  <div className={`w-9 h-9 rounded-lg ${i.bg} flex items-center justify-center shrink-0`}>{i.logo}</div>
                  <div className="min-w-0">
                    <div className="text-[12px] font-medium">{i.name}</div>
                    <div className="text-[10px] text-fg-4 truncate">{i.desc}</div>
                  </div>
                </div>
              ))}
            </div>
          </div>
        </div>

        {/* Right column */}
        <div className="space-y-4">
          {/* Org */}
          {o && (
            <div className="card">
              <div className="text-[12px] font-semibold mb-3">Organization</div>
              <div className="space-y-3">
                <div><div className="text-[10px] text-fg-4 uppercase tracking-wider mb-0.5">Name</div><div className="text-[13px] font-medium">{o.name}</div></div>
                <div><div className="text-[10px] text-fg-4 uppercase tracking-wider mb-0.5">Slug</div><div className="text-[13px] font-mono">{o.slug}</div></div>
                <div><div className="text-[10px] text-fg-4 uppercase tracking-wider mb-0.5">Role</div><RolePill role={o.role} /></div>
              </div>
            </div>
          )}

          {/* Appearance */}
          <div className="card">
            <div className="text-[12px] font-semibold mb-3">Appearance</div>
            <div className="flex items-center justify-between">
              <div>
                <div className="text-[13px] font-medium">Dark Mode</div>
                <div className="text-[10px] text-fg-4 mt-0.5">Toggle light / dark</div>
              </div>
              <button onClick={toggle} className="inline-flex items-center gap-1.5 text-[11px] font-medium px-3.5 py-2 rounded-xl bg-floor border border-border text-fg-2 hover:border-fg-5 transition-colors">
                {theme === 'dark' ? <Sun size={13} /> : <Moon size={13} />}
                {theme === 'dark' ? 'Light' : 'Dark'}
              </button>
            </div>
          </div>
        </div>
      </div>
    </>
  )
}
