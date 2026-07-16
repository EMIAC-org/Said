import { Outlet, Navigate } from 'react-router'
import { useAuth } from '../hooks/useAuth'
import { Sidebar } from './Sidebar'
import { Topbar } from './Topbar'
import { Loading } from './ui'

export function Layout() {
  const { token, loading, orgMissing } = useAuth()

  if (loading) return <div className="auth-wrap"><Loading /></div>
  if (!token) return <Navigate to="/login" replace />
  if (orgMissing) return <Navigate to="/onboarding" replace />

  return (
    <div className="app">
      <Sidebar />
      <div className="main">
        <Topbar />
        <div className="content">
          <div className="content-inner">
            <Outlet />
          </div>
        </div>
      </div>
    </div>
  )
}
