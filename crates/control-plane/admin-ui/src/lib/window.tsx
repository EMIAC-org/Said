import { createContext, useContext, useState, useCallback, type ReactNode } from 'react'

export type Win = 'today' | '7d' | '30d' | 'all'

export const WIN_LABEL: Record<Win, string> = {
  today: 'today',
  '7d': 'last 7 days',
  '30d': 'last 30 days',
  all: 'all time',
}

/** Maps a window to the backend `?days=` value. `all` = all-time sentinel. */
export function winDays(w: Win): string {
  return { today: '1', '7d': '7', '30d': '30', all: 'all' }[w]
}

const KEY = 'airnote:admin:window'

interface WinCtx {
  win: Win
  setWin: (w: Win) => void
}

const Ctx = createContext<WinCtx>(null!)

export function WindowProvider({ children }: { children: ReactNode }) {
  const [win, setWinState] = useState<Win>(() => (localStorage.getItem(KEY) as Win) || '7d')
  const setWin = useCallback((w: Win) => {
    localStorage.setItem(KEY, w)
    setWinState(w)
  }, [])
  return <Ctx.Provider value={{ win, setWin }}>{children}</Ctx.Provider>
}

export function useWindowRange() {
  return useContext(Ctx)
}
