//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Shared settings load/patch/save state, extracted from the old SettingsView
// so every Settings section (Capture/Notifications/Appearance/Advanced/
// Account) reads and writes the SAME RuntimeSettings instance instead of each
// re-fetching independently — a save in one section can never go stale
// against edits made in another while the modal is open.

'use client'

import { useEffect, useState } from 'react'
import type { RuntimeSettings } from '@/lib/settings'
import { load, mutate } from '@/lib/bridge'
import type { SaveStatus } from './fields'

export function useRuntimeSettings() {
  const [settings, setSettings] = useState<RuntimeSettings | null>(null)

  useEffect(() => {
    // get_settings (Rust) in the Tauri window, /api/settings in a browser.
    load<RuntimeSettings>('/api/settings', 'get_settings')
      .then(setSettings)
      .catch(() => {})
  }, [])

  function patch(changes: Partial<RuntimeSettings>) {
    setSettings(s => (s ? { ...s, ...changes } : s))
  }

  async function save(fields: Partial<RuntimeSettings>, setStatus?: (s: SaveStatus) => void) {
    setStatus?.('idle')
    try {
      // Dual-path: update_settings (Rust) in the app, /api/settings PUT in a browser.
      const updated = await mutate<RuntimeSettings>('/api/settings', 'update_settings', fields, 'PUT')
      setSettings(updated)
      setStatus?.('saved')
      if (setStatus) setTimeout(() => setStatus('idle'), 2000)
    } catch {
      setStatus?.('error')
      if (setStatus) setTimeout(() => setStatus('idle'), 3000)
    }
  }

  return { settings, setSettings, patch, save }
}
