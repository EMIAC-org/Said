import { Outlet, Navigate } from 'react-router'
import { useAuth } from '../hooks/useAuth'
import { Sidebar } from './Sidebar'
import { Topbar } from './Topbar'
import { Loading } from './States'

export function Layout() {
  const { token, loading, orgMissing } = useAuth()

  if (loading) return <div className="h-screen flex items-center justify-center bg-floor"><Loading /></div>
  if (!token) return <Navigate to="/login" replace />
  if (orgMissing) return <Navigate to="/onboarding" replace />

  return (
    <div className="flex h-screen overflow-hidden bg-sidebar">
      <Sidebar />
      <div className="flex-1 min-w-0 flex flex-col h-screen">
        {/* Topbar — sits on the floor */}
        <Topbar />
        {/* Mat — glass-strong panel, rounded top-left */}
        <div className="flex-1 min-h-0 pl-4">
          <div className="glass-strong rounded-tl-2xl h-full overflow-y-auto">
            <div className="max-w-[1200px] mx-auto px-10 py-7">
              <Outlet />
            </div>
          </div>
        </div>
      </div>
    </div>
  )
}
