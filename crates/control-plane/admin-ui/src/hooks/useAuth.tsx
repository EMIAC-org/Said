import { createContext, useContext, useState, useEffect, useCallback, type ReactNode } from 'react'
import { apiJson, api, setToken, clearToken, isAuthenticated } from '../api'
import type { User, Org } from '../types'

interface OrgResponse { org: Org }

interface AuthCtx {
  user: User | null
  org: OrgResponse | null
  token: boolean
  loading: boolean
  login: (email: string, password: string, signup?: boolean) => Promise<void>
  logout: () => void
  refreshOrg: () => void
}

const Ctx = createContext<AuthCtx>(null!)

export function AuthProvider({ children }: { children: ReactNode }) {
  const [user, setUser] = useState<User | null>(null)
  const [org, setOrg] = useState<OrgResponse | null>(null)
  const [token, setHasToken] = useState(isAuthenticated)
  const [loading, setLoading] = useState(true)

  const fetchData = useCallback(async () => {
    if (!isAuthenticated()) { setLoading(false); return }
    try {
      const [u, o] = await Promise.all([
        apiJson<User>('/v1/auth/me'),
        apiJson<{ org: Org }>('/v1/orgs/me').catch(() => null),
      ])
      setUser(u)
      setOrg(o)
    } catch {
      clearToken()
      setHasToken(false)
    }
    setLoading(false)
  }, [])

  useEffect(() => { fetchData() }, [fetchData])

  const login = useCallback(async (email: string, password: string, signup = false) => {
    const endpoint = signup ? '/v1/auth/signup' : '/v1/auth/login'
    const res = await api(endpoint, { method: 'POST', body: JSON.stringify({ email, password }) })
    const data = await res.json()
    if (!res.ok) throw new Error(data.error || 'Authentication failed')
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
  }, [])

  const refreshOrg = useCallback(() => {
    apiJson<{ org: Org }>('/v1/orgs/me').then(setOrg).catch(() => {})
  }, [])

  return (
    <Ctx.Provider value={{ user, org, token, loading, login, logout, refreshOrg }}>
      {children}
    </Ctx.Provider>
  )
}

export function useAuth() { return useContext(Ctx) }
