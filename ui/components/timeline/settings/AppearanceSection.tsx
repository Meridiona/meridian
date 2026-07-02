//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Settings → Appearance. The theme picker (ThemeSwatches, shared with the
// Toolbar) — previously bolted onto SettingsModal as an ad-hoc preamble
// above the flat SettingsView; now a proper tab like every other section.

'use client'

import { ThemeSwatches } from '../ThemeSwatches'

export function AppearanceSection() {
  return (
    <div className="max-w-[640px] flex flex-col gap-5">
      <div>
        <p className="mt-label" style={{ color: 'var(--color-state-proposal)' }}>Look &amp; feel</p>
        <h1 className="mt-title-lg mt-1.5" style={{ color: 'var(--t-title)' }}>Appearance</h1>
        <p className="mt-body-sm mt-2 max-w-[520px]" style={{ color: 'var(--t-muted)' }}>
          Switch between the Lilac, Blush and Ink themes. Applies instantly and persists across
          restarts — everywhere in the app shares this one setting.
        </p>
      </div>

      <div className="rounded-2xl p-5 flex items-center justify-between gap-6 bg-card"
        style={{ border: '1px solid var(--t-card-border)' }}>
        <div>
          <p className="mt-body-sm font-medium" style={{ color: 'var(--t-title)' }}>Theme</p>
          <p className="text-[11px] mt-0.5" style={{ color: 'var(--t-faint)' }}>Lilac · Blush · Ink</p>
        </div>
        <ThemeSwatches size={26} />
      </div>
    </div>
  )
}
