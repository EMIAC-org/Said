import { useNavigate } from 'react-router'
import { ArrowLeft, SearchX } from 'lucide-react'

export function NotFoundPage() {
  const navigate = useNavigate()

  return (
    <div className="min-h-[55vh] flex items-center justify-center">
      <div className="card max-w-[440px] text-center !p-8">
        <div className="w-12 h-12 rounded-2xl bg-accent-light text-accent flex items-center justify-center mx-auto mb-5">
          <SearchX size={22} />
        </div>
        <h1 className="text-[20px] font-semibold tracking-tight mb-2">Page not found</h1>
        <p className="text-[13px] text-fg-3 leading-relaxed mb-6">
          This admin page does not exist. Use the navigation or return to the dashboard.
        </p>
        <button onClick={() => navigate('/')} className="inline-flex items-center justify-center gap-2 text-[12px] font-semibold px-4 py-2.5 rounded-xl bg-accent text-accent-fg hover:bg-accent-hover transition-colors">
          <ArrowLeft size={14} /> Dashboard
        </button>
      </div>
    </div>
  )
}
