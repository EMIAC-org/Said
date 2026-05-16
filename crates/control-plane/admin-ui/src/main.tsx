import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { BrowserRouter, Routes, Route, Navigate } from 'react-router'
import { AuthProvider } from './hooks/useAuth'
import { Layout } from './components/Layout'
import { LoginPage } from './pages/LoginPage'
import { DashboardPage } from './pages/DashboardPage'
import { MeetingsPage } from './pages/MeetingsPage'
import { MeetingDetailPage } from './pages/MeetingDetailPage'
import { NewMeetingPage } from './pages/NewMeetingPage'
import { TeamPage } from './pages/TeamPage'
import { SettingsPage } from './pages/SettingsPage'
import { LiveMeetingPage } from './pages/LiveMeetingPage'
import './globals.css'

createRoot(document.getElementById('app')!).render(
  <StrictMode>
    <AuthProvider>
      <BrowserRouter basename={import.meta.env.BASE_URL}>
        <Routes>
          <Route path="/login" element={<LoginPage />} />
          <Route element={<Layout />}>
            <Route index element={<DashboardPage />} />
            <Route path="meetings" element={<MeetingsPage />} />
            <Route path="meetings/new" element={<NewMeetingPage />} />
            <Route path="meetings/:id" element={<MeetingDetailPage />} />
            <Route path="meetings/:id/live" element={<LiveMeetingPage />} />
            <Route path="team" element={<TeamPage />} />
            <Route path="settings" element={<SettingsPage />} />
          </Route>
          <Route path="*" element={<Navigate to="/" replace />} />
        </Routes>
      </BrowserRouter>
    </AuthProvider>
  </StrictMode>
)
