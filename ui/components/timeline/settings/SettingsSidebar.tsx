//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The Settings sidebar nav — ported from the Claude Design mock (Claude
// Design project b8656e29-ae04-4f69-b17f-d5fab4d00f3a, "Meridian Settings")
// but built on the app's real --t-*/--color-state-* tokens instead of the
// mock's hardcoded lilac hex, so it stays in sync with the lilac/blush/ink
// theme picker like every other Timeline surface. The mock's "Meridian Pro /
// Upgrade" upsell card is dropped — there's no such product, and this build
// only ships real features. "Back to Meridian" is dropped too — ModalShell's
// own header already has a close (×) affordance, so a second one inside the
// sidebar would be redundant.

'use client'

import type { IntegrationsResponse } from '@/lib/api-types'
import type { SettingsSection } from './types'

const NAV: { id: SettingsSection; label: string; glyph: string }[] = [
  { id: 'integrations', label: 'Integrations', glyph: '⚡' },
  { id: 'capture', label: 'Capture & Privacy', glyph: '◉' },
  { id: 'notifications', label: 'Notifications', glyph: '◔' },
  { id: 'appearance', label: 'Appearance', glyph: '◑' },
  { id: 'advanced', label: 'Advanced', glyph: '▤' },
  { id: 'account', label: 'Account', glyph: '◍' },
]

export function SettingsSidebar({ section, onSelect, integrations }: {
  section: SettingsSection
  onSelect: (s: SettingsSection) => void
  integrations: IntegrationsResponse | null
}) {
  const hasAnyConnected = !!integrations && (
    integrations.jira || integrations.linear || integrations.github ||
    integrations.trello || integrations.azure_devops
  )

  return (
    <div className="w-[220px] shrink-0 flex flex-col py-3.5 px-3 bg-panel"
      style={{ borderRight: '1px solid var(--t-hair)' }}>
      <p className="mt-label px-2 pb-2" style={{ color: 'var(--t-faint-2)' }}>Settings</p>
      {NAV.map(n => {
        const active = n.id === section
        const showBadge = n.id === 'integrations' && integrations !== null && !hasAnyConnected
        return (
          <button key={n.id} onClick={() => onSelect(n.id)}
            className="flex items-center gap-2.5 w-full rounded-lg px-2.5 py-2 mb-0.5 text-left"
            style={{
              border: 'none',
              cursor: 'pointer',
              font: "700 12.5px var(--font-pjs)",
              color: active ? 'var(--color-state-proposal)' : 'var(--t-muted)',
              background: active ? 'color-mix(in srgb, var(--color-state-proposal) 12%, transparent)' : 'transparent',
              boxShadow: active ? 'inset 0 0 0 1px color-mix(in srgb, var(--color-state-proposal) 24%, transparent)' : 'none',
            }}>
            <span className="inline-flex items-center justify-center rounded-md shrink-0"
              style={{
                width: 24, height: 24, fontSize: 12,
                background: active ? 'var(--t-card)' : 'var(--t-box)',
                color: active ? 'var(--color-state-proposal)' : 'var(--t-faint-2)',
              }} aria-hidden="true">
              {n.glyph}
            </span>
            <span className="flex-1">{n.label}</span>
            {showBadge && (
              <span className="rounded-full shrink-0"
                style={{ width: 6, height: 6, background: 'var(--color-state-proposal)' }}
                aria-label="No trackers connected" />
            )}
          </button>
        )
      })}
    </div>
  )
}
