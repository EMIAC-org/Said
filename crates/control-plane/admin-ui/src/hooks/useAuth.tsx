import { createContext, useContext, useState, useEffect, useCallback, type ReactNode } from 'react'
import { apiJson, api, setToken, clearToken, isAuthenticated, setActiveOrgId } from '../api'
import type { User, Org } from '../types'

interface OrgResponse { org: Org }

interface AuthMeResponse extends User {
  active_org_id?: string | null
  orgs?: Array<{
    id: string
    name: string
    slug: string
    role: string
    is_active?: boolean
  }>
  platform_admin?: boolean
  admin_orgs?: AdminOrg[]
}

export interface AdminOrg { id: string; name: string; slug: string }

interface AuthTokenResponse {
  token: string
}

interface AuthCtx {
  user: User | null
  org: OrgResponse | null
  orgMissing: boolean
  token: boolean
  loading: boolean
  login: (email: string, password: string, signup?: boolean) => Promise<void>
  logout: () => void
  refreshOrg: () => Promise<void>
  platformAdmin: boolean
  adminOrgs: AdminOrg[]
  adminScopeOrgId: string | null
  setAdminScopeOrgId: (orgId: string | null) => void
}

const Ctx = createContext<AuthCtx>(null!)

export function AuthProvider({ children }: { children: ReactNode }) {
  const [user, setUser] = useState<User | null>(null)
  const [org, setOrg] = useState<OrgResponse | null>(null)
  const [orgMissing, setOrgMissing] = useState(false)
  const [token, setHasToken] = useState(isAuthenticated)
  const [loading, setLoading] = useState(true)
  const [platformAdmin, setPlatformAdmin] = useState(false)
  const [adminOrgs, setAdminOrgs] = useState<AdminOrg[]>([])
  const [adminScopeOrgId, setAdminScopeOrgId] = useState<string | null>(null)

  const fetchOrgFromMe = useCallback(async (me: AuthMeResponse) => {
    const memberships = me.orgs ?? []
    if (memberships.length === 0) {
      setOrg(null)
      setOrgMissing(true)
      setActiveOrgId(null)
      return
    }
    const active =
      memberships.find(o => o.id === me.active_org_id) ??
      memberships.find(o => o.is_active) ??
      memberships[0]
    setActiveOrgId(active.id)
    setOrg({
      org: {
        id: active.id,
        name: active.name,
        slug: active.slug,
        role: active.role,
      },
    })
    setOrgMissing(false)
  }, [])

  const fetchData = useCallback(async () => {
    if (!isAuthenticated()) {
      setUser(null)
      setOrg(null)
      setOrgMissing(false)
      setActiveOrgId(null)
      setPlatformAdmin(false)
      setAdminOrgs([])
      setAdminScopeOrgId(null)
      setLoading(false)
      return
    }
    try {
      const me = await apiJson<AuthMeResponse>('/v1/auth/me')
      setUser(me)
      setPlatformAdmin(Boolean(me.platform_admin))
      setAdminOrgs(me.platform_admin ? (me.admin_orgs ?? []) : [])
      if (!me.platform_admin) setAdminScopeOrgId(null)
      await fetchOrgFromMe(me)
    } catch {
      clearToken()
      setHasToken(false)
      setUser(null)
      setOrg(null)
      setOrgMissing(false)
      setActiveOrgId(null)
      setPlatformAdmin(false)
      setAdminOrgs([])
      setAdminScopeOrgId(null)
    }
    setLoading(false)
  }, [fetchOrgFromMe])

  useEffect(() => { fetchData() }, [fetchData])

  const login = useCallback(async (email: string, password: string, signup = false) => {
    const endpoint = signup ? '/v1/auth/signup' : '/v1/auth/login'
    const data = await apiJson<AuthTokenResponse>(endpoint, {
      method: 'POST',
      body: JSON.stringify({ email, password }),
    })
    setToken(data.token)
    setHasToken(true)
    setLoading(true)
    await fetchData()
  }, [fetchData])

  const logout = useCallback(() => {
    const t = isAuthenticated()
    if (t) api('/v1/auth/logout', { method: 'POST' }).catch(() => {})
    clearToken()
    setHasToken(false)
    setUser(null)
    setOrg(null)
    setOrgMissing(false)
    setPlatformAdmin(false)
    setAdminOrgs([])
    setAdminScopeOrgId(null)
  }, [])

  const refreshOrg = useCallback(async () => {
    if (!isAuthenticated()) return
    const me = await apiJson<AuthMeResponse>('/v1/auth/me')
    setPlatformAdmin(Boolean(me.platform_admin))
    setAdminOrgs(me.platform_admin ? (me.admin_orgs ?? []) : [])
    if (!me.platform_admin) setAdminScopeOrgId(null)
    await fetchOrgFromMe(me)
  }, [fetchOrgFromMe])

  return (
    <Ctx.Provider value={{
      user, org, orgMissing, token, loading, login, logout, refreshOrg,
      platformAdmin, adminOrgs, adminScopeOrgId, setAdminScopeOrgId,
    }}>
      {children}
    </Ctx.Provider>
  )
}

export function useAuth() { return useContext(Ctx) }
