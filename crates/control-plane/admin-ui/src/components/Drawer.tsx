import { createContext, useContext, useState, useCallback, type ReactNode } from 'react'

interface DrawerContent {
  head: ReactNode
  body: ReactNode
}

interface DrawerCtx {
  open: (content: DrawerContent) => void
  close: () => void
}

const Ctx = createContext<DrawerCtx>(null!)

export function DrawerProvider({ children }: { children: ReactNode }) {
  const [content, setContent] = useState<DrawerContent | null>(null)
  const [visible, setVisible] = useState(false)

  const open = useCallback((c: DrawerContent) => {
    setContent(c)
    // next frame → transition in
    requestAnimationFrame(() => setVisible(true))
  }, [])

  const close = useCallback(() => setVisible(false), [])

  return (
    <Ctx.Provider value={{ open, close }}>
      {children}
      <div className={`scrim${visible ? ' open' : ''}`} onClick={close} />
      <aside className={`drawer${visible ? ' open' : ''}`}>
        {content && (
          <>
            <div className="drawer-head">{content.head}</div>
            <div className="drawer-body">{content.body}</div>
          </>
        )}
      </aside>
    </Ctx.Provider>
  )
}

export function useDrawer() {
  return useContext(Ctx)
}

export function DrawerClose({ onClick }: { onClick: () => void }) {
  return (
    <div className="icon-btn" onClick={onClick} aria-label="Close">
      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8">
        <path d="M6 6l12 12M18 6L6 18" />
      </svg>
    </div>
  )
}
