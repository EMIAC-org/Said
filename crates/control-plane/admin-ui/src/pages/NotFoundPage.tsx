import { Link } from 'react-router'

export function NotFoundPage() {
  return (
    <div className="auth-wrap">
      <div style={{ textAlign: 'center' }}>
        <div style={{ fontSize: 48, letterSpacing: '-0.04em', color: 'var(--ink)' }}>404</div>
        <p style={{ fontSize: 13.5, color: 'var(--muted)', margin: '8px 0 18px' }}>This page doesn’t exist.</p>
        <Link to="/" className="btn primary">Back to Overview</Link>
      </div>
    </div>
  )
}
