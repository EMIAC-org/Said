import { StrictMode, type ReactNode } from 'react'
import { createRoot } from 'react-dom/client'
import { BrowserRouter, Routes, Route } from 'react-router'
import { AuthProvider } from './hooks/useAuth'
import { WindowProvider } from './lib/window'
import { DrawerProvider } from './components/Drawer'
import { SearchProvider } from './components/Search'
import { Layout } from './components/Layout'
import { LoginPage } from './pages/LoginPage'
import { OnboardingPage } from './pages/OnboardingPage'
import { OverviewPage } from './pages/OverviewPage'
import { RunsPage } from './pages/RunsPage'
import { PeoplePage } from './pages/PeoplePage'
import { PersonDetailPage } from './pages/PersonDetailPage'
import { MeetingsPage } from './pages/MeetingsPage'
import { NotFoundPage } from './pages/NotFoundPage'
import './globals.css'

function Shell({ children }: { children: ReactNode }) {
  return (
    <WindowProvider>
      <SearchProvider>
        <DrawerProvider>{children}</DrawerProvider>
      </SearchProvider>
    </WindowProvider>
  )
}

const isAdminMount = window.location.pathname === '/admin' || window.location.pathname.startsWith('/admin/')

createRoot(document.getElementById('app')!).render(
  <StrictMode>
    <AuthProvider>
      <BrowserRouter basename={isAdminMount ? '/admin' : undefined}>
        {isAdminMount ? (
          <Routes>
            <Route path="/login" element={<LoginPage />} />
            <Route path="/onboarding" element={<OnboardingPage />} />
            <Route element={<Shell><Layout /></Shell>}>
              <Route index element={<OverviewPage />} />
              <Route path="runs" element={<RunsPage />} />
              <Route path="people" element={<PeoplePage />} />
              <Route path="people/:id" element={<PersonDetailPage />} />
              <Route path="meetings" element={<MeetingsPage />} />
              <Route path="*" element={<NotFoundPage />} />
            </Route>
          </Routes>
        ) : (
          <Routes>
            <Route path="*" element={<NotFoundPage />} />
          </Routes>
        )}
      </BrowserRouter>
    </AuthProvider>
  </StrictMode>
)
