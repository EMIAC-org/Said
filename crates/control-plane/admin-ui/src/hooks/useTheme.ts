import { useState, useCallback, useEffect } from 'react'

type Theme = 'light' | 'dark'
const THEME_KEY = 'airnote:theme'
const LEGACY_THEME_KEY = 'said:theme'

function getStoredTheme(): Theme {
  const theme = localStorage.getItem(THEME_KEY) as Theme | null
  if (theme) return theme

  const legacyTheme = localStorage.getItem(LEGACY_THEME_KEY) as Theme | null
  if (legacyTheme) {
    localStorage.setItem(THEME_KEY, legacyTheme)
    localStorage.removeItem(LEGACY_THEME_KEY)
    return legacyTheme
  }

  return 'light'
}

export function useTheme() {
  const [theme, setThemeState] = useState<Theme>(() => getStoredTheme())

  useEffect(() => {
    document.documentElement.setAttribute('data-theme', theme)
  }, [theme])

  const setTheme = useCallback((t: Theme) => {
    localStorage.setItem(THEME_KEY, t)
    localStorage.removeItem(LEGACY_THEME_KEY)
    setThemeState(t)
  }, [])

  const toggle = useCallback(() => {
    setTheme(theme === 'dark' ? 'light' : 'dark')
  }, [theme, setTheme])

  return { theme, setTheme, toggle }
}
