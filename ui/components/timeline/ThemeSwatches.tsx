//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The three theme swatches, shared by the Toolbar and the Settings modal. Each
// click applies the palette immediately (optimistic, no flash) then persists it
// via update_settings — the same optimistic-then-persist flow SettingsView uses
// for every other field. Current selection is read from get_settings on mount.

'use client'

import { useEffect, useState } from 'react'
import { load, mutate } from '@/lib/bridge'
import type { RuntimeSettings } from '@/lib/settings'
import { applyTheme, THEME_IDS, type MeridianTheme } from '@/lib/theme'

// The gradient shown on each swatch — mirrors each palette's --chip token so a
// swatch previews the palette it selects. (Static: reading a CSS var of an
// inactive theme block isn't possible without mounting it.)
const SWATCH_GRADIENT: Record<MeridianTheme, string> = {
  lilac: 'linear-gradient(135deg,#C4B5FD,#A5B4FC)',
  blush: 'linear-gradient(135deg,#C4B5FD,#8B7FDB)',
  ink: 'linear-gradient(135deg,#3A3470,#0E0B1F)',
}

export function ThemeSwatches({ size = 22 }: { size?: number }) {
  const [current, setCurrent] = useState<MeridianTheme | null>(null)

  useEffect(() => {
    load<RuntimeSettings>('/api/settings', 'get_settings')
      .then(s => setCurrent(s.theme))
      .catch(() => {})
  }, [])

  function pick(theme: MeridianTheme) {
    setCurrent(theme)
    applyTheme(theme)
    mutate<RuntimeSettings>('/api/settings', 'update_settings', { theme }, 'PUT').catch(() => {})
  }

  return (
    <div className="flex items-center gap-1.5">
      {THEME_IDS.map(id => (
        <button key={id} onClick={() => pick(id)} aria-label={`${id} theme`} title={id}
          className="rounded-full transition-transform active:scale-90"
          style={{
            width: size, height: size,
            background: SWATCH_GRADIENT[id],
            border: current === id ? '2px solid var(--t-title)' : '2px solid transparent',
            boxShadow: current === id ? '0 0 0 1px var(--t-ctrl-border)' : 'none',
          }} />
      ))}
    </div>
  )
}
