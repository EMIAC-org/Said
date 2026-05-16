const API = ''

function getToken(): string | null {
  return localStorage.getItem('said:admin:token')
}

export function setToken(t: string) {
  localStorage.setItem('said:admin:token', t)
}

export function clearToken() {
  localStorage.removeItem('said:admin:token')
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
