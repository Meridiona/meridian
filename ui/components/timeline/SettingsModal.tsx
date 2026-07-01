//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Settings modal — wraps the existing SettingsView unchanged (all Tauri
// round-trips intact) and prepends an Appearance section with the theme picker
// (SettingsView has no appearance section of its own, so this is additive, not
// a duplicate). The theme picker shares ThemeSwatches with the Toolbar.

'use client'

import SettingsView from '@/components/views/SettingsView'
import { ModalShell } from './ModalShell'
import { ThemeSwatches } from './ThemeSwatches'

export function SettingsModal({ onClose }: { onClose: () => void }) {
  return (
    <ModalShell title="Settings" onClose={onClose} maxWidth={720}>
      <div className="rounded-xl p-5 mb-5 bg-card flex items-center justify-between gap-6"
        style={{ border: '1px solid var(--t-card-border)' }}>
        <div>
          <p className="mt-label" style={{ color: 'var(--t-faint)' }}>Appearance</p>
          <p className="mt-title text-title mt-1">Theme</p>
          <p className="mt-body-sm mt-0.5" style={{ color: 'var(--t-muted)' }}>Applies instantly and persists across restarts.</p>
        </div>
        <ThemeSwatches size={26} />
      </div>
      <SettingsView />
    </ModalShell>
  )
}
