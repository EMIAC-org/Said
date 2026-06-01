import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { BrowserRouter, Routes, Route } from 'react-router'
import { AuthProvider } from './hooks/useAuth'
import { Layout } from './components/Layout'
import { LoginPage } from './pages/LoginPage'
import { DashboardPage } from './pages/DashboardPage'
import { MeetingsPage } from './pages/MeetingsPage'
import { MeetingDetailPage } from './pages/MeetingDetailPage'
import { NewMeetingPage } from './pages/NewMeetingPage'
import { TeamPage } from './pages/TeamPage'
import { DesktopPage } from './pages/DesktopPage'
import { VocabularyPage } from './pages/VocabularyPage'
import { BugsPage } from './pages/BugsPage'
import { DiagnosticsPage } from './pages/DiagnosticsPage'
import { SettingsPage } from './pages/SettingsPage'
import { LiveMeetingPage } from './pages/LiveMeetingPage'
import { OnboardingPage } from './pages/OnboardingPage'
import { NotFoundPage } from './pages/NotFoundPage'
import './globals.css'

createRoot(document.getElementById('app')!).render(
  <StrictMode>
    <AuthProvider>
      <BrowserRouter basename={import.meta.env.BASE_URL}>
        <Routes>
          <Route path="/login" element={<LoginPage />} />
          <Route path="/onboarding" element={<OnboardingPage />} />
          <Route element={<Layout />}>
            <Route index element={<DashboardPage />} />
            <Route path="meetings" element={<MeetingsPage />} />
            <Route path="meetings/new" element={<NewMeetingPage />} />
            <Route path="meetings/:id" element={<MeetingDetailPage />} />
            <Route path="meetings/:id/live" element={<LiveMeetingPage />} />
            <Route path="team" element={<TeamPage />} />
            <Route path="desktop" element={<DesktopPage />} />
            <Route path="vocabulary" element={<VocabularyPage />} />
            <Route path="bugs" element={<BugsPage />} />
            <Route path="diagnostics" element={<DiagnosticsPage />} />
            <Route path="settings" element={<SettingsPage />} />
            <Route path="*" element={<NotFoundPage />} />
          </Route>
          <Route path="*" element={<NotFoundPage />} />
        </Routes>
      </BrowserRouter>
    </AuthProvider>
  </StrictMode>
)
