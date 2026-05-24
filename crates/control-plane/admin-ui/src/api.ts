const API = ''
const AUTH_TOKEN_KEY = 'airnote:admin:token'
const LEGACY_AUTH_TOKEN_KEY = 'said:admin:token'

export function getToken(): string | null {
  const token = localStorage.getItem(AUTH_TOKEN_KEY)
  if (token) return token

  const legacyToken = localStorage.getItem(LEGACY_AUTH_TOKEN_KEY)
  if (legacyToken) {
    localStorage.setItem(AUTH_TOKEN_KEY, legacyToken)
    localStorage.removeItem(LEGACY_AUTH_TOKEN_KEY)
  }
  return legacyToken
}

export function setToken(t: string) {
  localStorage.setItem(AUTH_TOKEN_KEY, t)
  localStorage.removeItem(LEGACY_AUTH_TOKEN_KEY)
}

export function clearToken() {
  localStorage.removeItem(AUTH_TOKEN_KEY)
  localStorage.removeItem(LEGACY_AUTH_TOKEN_KEY)
}

export function isAuthenticated(): boolean {
  return !!getToken()
}

export async function api(path: string, opts: RequestInit = {}): Promise<Response> {
  const token = getToken()
  const headers: Record<string, string> = { ...(opts.headers as Record<string, string>) }
  if (opts.body && typeof opts.body === 'string') headers['Content-Type'] = 'application/json'
  if (token) headers['Authorization'] = 'Bearer ' + token
  return fetch(API + path, { ...opts, headers })
}

export async function apiJson<T = unknown>(path: string, opts: RequestInit = {}): Promise<T> {
  const res = await api(path, opts)
  if (res.status === 204) return null as T
  const text = await res.text()
  let data: T
  try {
    data = JSON.parse(text)
  } catch {
    throw new Error(text || `Request failed (${res.status})`)
  }
  if (!res.ok) throw new Error((data as { error?: string }).error || `Request failed (${res.status})`)
  return data
}
