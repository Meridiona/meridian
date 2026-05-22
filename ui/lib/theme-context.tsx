// meridian — normalises screenpipe activity into structured app sessions
'use client'

import React, { createContext, useContext, useEffect, useState, useCallback } from 'react'
import { TooltipProvider } from '@radix-ui/react-tooltip'

export const ACCENT_PRESETS = ['#C4822A', '#2A6FDB', '#1F8A5B', '#0D0C0A'] as const

export interface ThemeState {
  dark: boolean
  accent: string
  density: 'compact' | 'regular' | 'comfy'
  tone: 'terse' | 'detailed'
}

interface ThemeContextValue extends ThemeState {
  setDark: (v: boolean) => void
  setAccent: (v: string) => void
  setDensity: (v: ThemeState['density']) => void
  setTone: (v: ThemeState['tone']) => void
}

const DEFAULTS: ThemeState = {
  dark: false,
  accent: '#C4822A',
  density: 'regular',
  tone: 'terse',
}

const ThemeContext = createContext<ThemeContextValue>({
  ...DEFAULTS,
  setDark: () => {},
  setAccent: () => {},
  setDensity: () => {},
  setTone: () => {},
})

function hexA(hex: string, a: number): string {
  const h = hex.replace('#', '')
  const r = parseInt(h.substring(0, 2), 16)
  const g = parseInt(h.substring(2, 4), 16)
  const b = parseInt(h.substring(4, 6), 16)
  return `rgba(${r},${g},${b},${a})`
}

function load(): ThemeState {
  if (typeof window === 'undefined') return DEFAULTS
  try {
    const raw = localStorage.getItem('meridian-theme')
    return raw ? { ...DEFAULTS, ...JSON.parse(raw) } : DEFAULTS
  } catch {
    return DEFAULTS
  }
}

function applyToRoot(state: ThemeState) {
  const root = document.documentElement
  root.classList.toggle('dark', state.dark)
  root.classList.remove('density-compact', 'density-regular', 'density-comfy')
  root.classList.add(`density-${state.density}`)
  root.style.setProperty('--accent', state.accent)
  root.style.setProperty('--accent-soft', hexA(state.accent, state.dark ? 0.18 : 0.13))
  root.style.setProperty('--tint', hexA(state.accent, state.dark ? 0.10 : 0.06))
  root.style.setProperty('--live', state.accent)
}

export function ThemeProvider({ children }: { children: React.ReactNode }) {
  const [state, setState] = useState<ThemeState>(DEFAULTS)

  useEffect(() => {
    const saved = load()
    setState(saved)
    applyToRoot(saved)
  }, [])

  const update = useCallback((patch: Partial<ThemeState>) => {
    setState(prev => {
      const next = { ...prev, ...patch }
      applyToRoot(next)
      try { localStorage.setItem('meridian-theme', JSON.stringify(next)) } catch {}
      return next
    })
  }, [])

  const value: ThemeContextValue = {
    ...state,
    setDark:    (v) => update({ dark: v }),
    setAccent:  (v) => update({ accent: v }),
    setDensity: (v) => update({ density: v }),
    setTone:    (v) => update({ tone: v }),
  }

  return (
    <ThemeContext.Provider value={value}>
      <TooltipProvider delayDuration={300}>
        {children}
      </TooltipProvider>
    </ThemeContext.Provider>
  )
}

export function useTheme() {
  return useContext(ThemeContext)
}
