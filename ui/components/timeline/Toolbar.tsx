//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The 60px top toolbar (STYLESHEET.md §7): date navigation on the left, the
// Meridian nav pill centered, and the theme swatches / auto-derived
// Connected-Solo pill / capture pause-resume pill on the right. A 3-column
// grid (1fr / auto / 1fr) keeps the brand lockup truly centered regardless of
// how wide the left/right content is — same technique as the tray popover's
// header. The connected/solo pill is NOT a manual toggle — it reflects real
// get_integrations state from useTimelineData.
//
// The centered nav pill is ported from the Meridian Timeline design mock
// (Claude Design project b8656e29-ae04-4f69-b17f-d5fab4d00f3a): a solid dark
// lockup — gradient logo mark, wordmark, Timeline/Integrations/Settings —
// deliberately independent of the light/blush/ink surface theme (same
// "consistent across themes" rule the mock's own `SURF`/`THEMES` split uses).
// The mock's Timeline/Chat toggle only has one live surface today, so Chat is
// dropped and Timeline renders as a static active label rather than an inert
// control. "Integrations" opens Settings pre-scrolled to the Integrations tab
// (see SettingsModal); "Settings" opens the same modal, which also makes the
// old standalone settings-gear button on the right redundant — removed in
// favor of this.

'use client'

import { useCallback, useEffect, useState } from 'react'
import { load } from '@/lib/bridge'
import { formatDayLabel } from './types'
import { ThemeSwatches } from './ThemeSwatches'
import { ProviderIcon } from '@/components/ProviderIcon'
import type { SettingsSection } from './settings/types'

export function Toolbar({
  day, isToday, onShiftDay, isSolo, connectedProviderName, connectedProviderId, onOpenSettings, onOpenReport,
}: {
  day: string
  isToday: boolean
  onShiftDay: (delta: number) => void
  isSolo: boolean
  connectedProviderName: string | null
  connectedProviderId: string | null
  onOpenSettings: (section?: SettingsSection) => void
  onOpenReport: () => void
}) {
  const [running, setRunning] = useState<boolean | null>(null)

  const refreshStatus = useCallback(() => {
    load<{ running: boolean }>('/api/daemon/status', 'get_daemon_status')
      .then(s => setRunning(s.running))
      .catch(() => setRunning(null))
  }, [])

  useEffect(() => {
    refreshStatus()
    const id = setInterval(refreshStatus, 30_000)
    return () => clearInterval(id)
  }, [refreshStatus])

  const connectionLabel = isSolo ? 'Solo' : connectedProviderName ?? 'Connected'

  return (
    <div className="grid items-center gap-4 px-6 shrink-0 border-b"
      style={{ height: 60, gridTemplateColumns: '1fr auto 1fr', borderColor: 'var(--t-hair)', background: 'var(--toolbar-bg)' }}>
      <div className="flex items-center gap-4 min-w-0">
        {/* date nav */}
        <div className="flex items-center gap-1">
          <NavBtn glyph="‹" label="Previous day" onClick={() => onShiftDay(-1)} />
          <span className="mt-toolbar-date px-2 min-w-32 text-center whitespace-nowrap" style={{ color: 'var(--t-title)' }}>
            {isToday ? 'Today' : formatDayLabel(day)}
          </span>
          <NavBtn glyph="›" label="Next day" onClick={() => onShiftDay(1)} disabled={isToday} />
        </div>
      </div>

      {/* nav pill — centered regardless of left/right content width. */}
      <div className="justify-self-center">
        <MeridianNavPill onOpenSettings={onOpenSettings} onOpenReport={onOpenReport} />
      </div>

      <div className="ml-auto flex items-center gap-4">
        <ThemeSwatches />
        <span className="w-px h-5" style={{ background: 'var(--t-hair)' }} />
        <span className="inline-flex items-center gap-1.5 rounded-full px-3 py-1.5 bg-ctrl"
          style={{ border: '1px solid var(--t-ctrl-border)' }}>
          {connectedProviderId
            ? <ProviderIcon provider={connectedProviderId} size={12} />
            : <span className="inline-block w-1.5 h-1.5 rounded-full"
                style={{ background: isSolo ? 'var(--t-faint)' : 'var(--color-state-proposal)' }} />}
          <span className="mt-body-sm" style={{ color: 'var(--t-muted)' }}>{connectionLabel}</span>
        </span>

        {/* capture status pill — right side, after the connected/solo pill.
            Display-only: reflects daemon status, not a manual toggle. */}
        <span className="inline-flex items-center gap-1.5 rounded-full px-3 py-1.5 bg-ctrl shrink-0"
          style={{ border: '1px solid var(--t-ctrl-border)', opacity: running === null ? 0.6 : 1 }}>
          <span className="inline-block w-2 h-2 rounded-full"
            style={{ background: running ? 'var(--color-state-approved)' : 'var(--color-state-pending)' }} />
          <span className="mt-body-sm" style={{ color: 'var(--t-muted)' }}>
            {running === null ? 'Capture' : running ? 'Capturing' : 'Paused'}
          </span>
        </span>
      </div>
    </div>
  )
}

/** The Meridian brand mark — renders the app icon (ui/app/icon.png, copied to
 *  ui/public/meridian-logo.png so it's servable as a plain asset without
 *  colliding with Next's app/icon.png favicon-route convention). Exported so
 *  any other surface using the Meridian brand mark (e.g. HourBadges'
 *  generating-hour takeover card) renders the identical asset, not a
 *  re-derived one. `size` defaults to the toolbar's 15px. The icon's glyph
 *  occupies ~60% of its source canvas (app-icon safe-area padding, same as
 *  the tray popover's .brand-mark), so it's scaled up via backgroundSize
 *  rather than shown at its native padded ratio. */
export function MeridianMark({ size = 15 }: { size?: number }) {
  return (
    <span aria-hidden="true" className="shrink-0" style={{
      width: size,
      height: size,
      borderRadius: size / 3,
      backgroundImage: 'url(/meridian-logo.png)',
      backgroundSize: '166% 166%',
      backgroundPosition: 'center',
      backgroundRepeat: 'no-repeat',
    }} />
  )
}

/** Shared look for the pill's nav items (Timeline/Integrations) — active vs
 *  inactive, matching the mock's `_pillStyle(active, DARK=true)`. */
function NavPillItem({ active, onClick, children }: {
  active: boolean
  onClick?: () => void
  children: React.ReactNode
}) {
  const style: React.CSSProperties = {
    display: 'flex',
    alignItems: 'center',
    border: 'none',
    borderRadius: 999,
    padding: '8px 14px',
    background: active ? 'rgba(255,255,255,.14)' : 'transparent',
    color: active ? '#FFFFFF' : 'rgba(255,255,255,.62)',
    cursor: onClick ? 'pointer' : 'default',
  }
  return onClick
    ? <button onClick={onClick} className="mt-navpill-item" style={style}>{children}</button>
    : <span className="mt-navpill-item" style={style}>{children}</span>
}

/** The gear icon used by the pill's Settings item — ported verbatim from the
 *  design mock's inline SVG. */
function SettingsGlyph() {
  return (
    <svg width="12" height="12" viewBox="0 0 22 22" fill="none" aria-hidden="true">
      <path d="M9 2.5 A1.7 1.7 0 0 1 13 2.5 L13.3 4.4 A6 6 0 0 1 15 5.4 L16.8 4.7 A1.7 1.7 0 0 1 18.8 7.7 L17.5 9.1 A6 6 0 0 1 17.5 11 L18.8 12.4 A1.7 1.7 0 0 1 16.8 15.4 L15 14.7 A6 6 0 0 1 13.3 15.7 L13 17.6 A1.7 1.7 0 0 1 9 17.6 L8.7 15.7 A6 6 0 0 1 7 14.7 L5.2 15.4 A1.7 1.7 0 0 1 3.2 12.4 L4.5 11 A6 6 0 0 1 4.5 9.1 L3.2 7.7 A1.7 1.7 0 0 1 5.2 4.7 L7 5.4 A6 6 0 0 1 8.7 4.4 Z"
        stroke="currentColor" strokeWidth="1.7" strokeLinejoin="round" />
      <circle cx="11" cy="10.05" r="2.5" stroke="currentColor" strokeWidth="1.7" />
    </svg>
  )
}

/** The centered brand navbar: gradient mark + "Meridian" wordmark + Timeline
 *  (static active label — the app's only surface) + Integrations/Report/
 *  Settings. Integrations pre-selects the Integrations tab, Settings opens
 *  on the default tab (both open SettingsModal); Report opens the
 *  ReportModal get-in-touch card (see MeridianTimelineShell). Solid dark
 *  lockup, unaffected by the light/blush/ink surface theme — ported from the
 *  design mock's pill navbar (`pillBarStyle`/`pillBrandStyle`/`pillSettingsStyle`). */
function MeridianNavPill({ onOpenSettings, onOpenReport }: {
  onOpenSettings: (section?: SettingsSection) => void
  onOpenReport: () => void
}) {
  return (
    <div className="flex items-center" style={{
      gap: 2,
      padding: '5px 6px 5px 16px',
      borderRadius: 999,
      background: 'var(--mer-pill-bg)',
      border: '1px solid var(--mer-pill-border)',
      boxShadow: 'var(--mer-pill-shadow)',
    }}>
      <MeridianMark />
      <span className="mt-navpill-brand" style={{ color: '#FFFFFF', margin: '0 14px 0 9px' }}>Meridian</span>
      <NavPillItem active>Timeline</NavPillItem>
      <NavPillItem active={false} onClick={() => onOpenSettings('integrations')}>Integrations</NavPillItem>
      <NavPillItem active={false} onClick={onOpenReport}>Report</NavPillItem>
      <button onClick={() => onOpenSettings()} aria-label="Settings" className="mt-navpill-item"
        style={{
          display: 'flex', alignItems: 'center', gap: 6, borderRadius: 999,
          padding: '8px 15px', marginLeft: 6, background: 'rgba(255,255,255,.1)',
          border: '1px solid rgba(255,255,255,.14)', color: '#FFFFFF', cursor: 'pointer',
        }}>
        <SettingsGlyph />
        Settings
      </button>
    </div>
  )
}

function NavBtn({ glyph, label, onClick, disabled }: {
  glyph: string; label: string; onClick: () => void; disabled?: boolean
}) {
  return (
    <button onClick={onClick} disabled={disabled} aria-label={label}
      className="inline-flex items-center justify-center rounded-md bg-ctrl"
      style={{ width: 28, height: 28, border: '1px solid var(--t-ctrl-border)', color: disabled ? 'var(--t-faint-2)' : 'var(--t-muted)' }}>
      <span className="text-[15px] leading-none">{glyph}</span>
    </button>
  )
}
