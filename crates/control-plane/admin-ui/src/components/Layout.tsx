import { Outlet, Navigate } from 'react-router'
import { useAuth } from '../hooks/useAuth'
import { Sidebar } from './Sidebar'
import { Topbar } from './Topbar'
import { Loading } from './States'

export function Layout() {
  const { token, loading } = useAuth()

  if (loading) return <div className="h-screen flex items-center justify-center bg-floor"><Loading /></div>
  if (!token) return <Navigate to="/login" replace />

  return (
    <div className="flex h-screen overflow-hidden bg-floor">
      <Sidebar />
      <div className="flex-1 min-w-0 flex flex-col h-screen">
        {/* Topbar — sits on the floor */}
        <Topbar />
        {/* Mat — raised surface, rounded top-left, flush right & bottom */}
        <div className="flex-1 min-h-0 pl-4">
          <div className="bg-surface rounded-tl-[20px] h-full overflow-y-auto shadow-[0_0_0_1px_rgba(0,0,0,0.04)] dark:shadow-[inset_0_1px_0_0_rgba(255,255,255,0.04),0_0_0_1px_rgba(255,255,255,0.03)]">
            <div className="max-w-[1200px] mx-auto px-10 py-7">
              <Outlet />
            </div>
          </div>
        </div>
      </div>
    </div>
  )
}
