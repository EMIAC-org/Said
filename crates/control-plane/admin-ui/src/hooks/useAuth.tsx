import { createContext, useContext, useState, useEffect, useCallback, type ReactNode } from 'react'
import { apiJson, api, setToken, clearToken, isAuthenticated } from '../api'
import type { User, Org } from '../types'

interface OrgResponse { org: Org }

interface AuthCtx {
  user: User | null
  org: OrgResponse | null
  orgMissing: boolean
  token: boolean
  loading: boolean
  login: (email: string, password: string, signup?: boolean) => Promise<void>
  logout: () => void
  refreshOrg: () => Promise<void>
}

const Ctx = createContext<AuthCtx>(null!)

export function AuthProvider({ children }: { children: ReactNode }) {
  const [user, setUser] = useState<User | null>(null)
  const [org, setOrg] = useState<OrgResponse | null>(null)
  const [orgMissing, setOrgMissing] = useState(false)
  const [token, setHasToken] = useState(isAuthenticated)
  const [loading, setLoading] = useState(true)

  const fetchOrg = useCallback(async () => {
    try {
      const res = await api('/v1/orgs/me')
      if (res.status === 404) {
        setOrg(null)
        setOrgMissing(true)
        return
      }
      const text = await res.text()
      let data: { org: Org; error?: string }
      try {
        data = JSON.parse(text)
      } catch {
        throw new Error(text || `Request failed (${res.status})`)
      }
      if (!res.ok) throw new Error(data.error || `Request failed (${res.status})`)
      const nextOrg = data
      setOrg(nextOrg)
      setOrgMissing(false)
    } catch {
      setOrg(null)
      setOrgMissing(false)
    }
  }, [])

  const fetchData = useCallback(async () => {
    if (!isAuthenticated()) {
      setUser(null)
      setOrg(null)
      setOrgMissing(false)
      setLoading(false)
      return
    }
    try {
      const u = await apiJson<User>('/v1/auth/me')
      setUser(u)
      await fetchOrg()
    } catch {
      clearToken()
      setHasToken(false)
      setUser(null)
      setOrg(null)
      setOrgMissing(false)
    }
    setLoading(false)
  }, [fetchOrg])

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
    setOrgMissing(false)
  }, [])

  const refreshOrg = useCallback(fetchOrg, [fetchOrg])

  return (
    <Ctx.Provider value={{ user, org, orgMissing, token, loading, login, logout, refreshOrg }}>
      {children}
    </Ctx.Provider>
  )
}

export function useAuth() { return useContext(Ctx) }
