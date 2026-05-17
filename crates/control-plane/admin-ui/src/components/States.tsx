export function Loading() {
  return (
    <div className="flex items-center justify-center py-16 gap-2.5 text-fg-3 text-sm">
      <div className="spinner" /> Loading...
    </div>
  )
}

export function Empty({ title, message }: { title: string; message?: string }) {
  return (
    <div className="card text-center py-12">
      <h3 className="text-[15px] font-semibold text-fg mb-1.5">{title}</h3>
      {message && <p className="text-sm text-fg-3">{message}</p>}
    </div>
  )
}

export function ErrorBox({ title, message }: { title: string; message: string }) {
  return (
    <div className="bg-live-bg border border-live/20 rounded-xl p-5 mb-5">
      <h3 className="text-sm font-semibold text-live mb-1">{title}</h3>
      <p className="text-xs text-fg-3">{message}</p>
    </div>
  )
}
