import { useNavigate } from 'react-router'
import { ArrowLeft, SearchX } from 'lucide-react'

export function NotFoundPage() {
  const navigate = useNavigate()

  return (
    <div className="min-h-[55vh] flex items-center justify-center">
      <div className="card max-w-[440px] text-center !p-8">
        <div className="w-12 h-12 rounded-xl bg-surface-4 text-accent flex items-center justify-center mx-auto mb-5">
          <SearchX size={22} />
        </div>
        <h1 className="text-[20px] font-semibold tracking-tight mb-2">Page not found</h1>
        <p className="text-[13px] text-fg-3 leading-relaxed mb-6">
          This admin page does not exist. Use the navigation or return to the dashboard.
        </p>
        <button onClick={() => navigate('/')} className="inline-flex items-center justify-center gap-2 text-[12px] font-semibold px-4 h-9 rounded-lg bg-[hsl(0_0%_98%)] text-[hsl(240_8%_8%)] hover:opacity-90 transition-all">
          <ArrowLeft size={14} /> Dashboard
        </button>
      </div>
    </div>
  )
}
