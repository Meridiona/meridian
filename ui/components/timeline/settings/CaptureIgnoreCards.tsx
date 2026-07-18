//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Settings → Capture & Privacy → the two "ignore" cards: apps and websites
// Meridian must never watch. Quick-touch by design - tapping a chip ignores (or
// un-ignores) and saves immediately, no separate Save button. The lists write to
// RuntimeSettings.ignored_apps / ignored_urls; the tray's update_settings refreshes
// the live capture filter on save, so a change takes effect on the next captured
// frame with no capture restart. Going forward only - already-recorded history is
// untouched (matches the incognito / password-manager posture).
//
// Each card also surfaces what Meridian ALREADY skips by default, so "what is
// being ignored" is one honest picture rather than only the user's own entries:
// - apps: password managers + Meridian's own windows are never captured.
// - websites: streaming services are skipped while the "Skip streaming services"
//   toggle above is on (a separate, built-in mechanism - shown here read-only so
//   the ignored-sites picture is complete).
//
// Split out of CaptureSection.tsx to keep each file focused; rendered by it.

'use client'

import { useEffect, useState } from 'react'
import { load } from '@/lib/bridge'
import type { CaptureApp } from '@/lib/api-types'
import type { RuntimeSettings } from '@/lib/settings'
import { AppGlyph } from '@/components/atoms'
import { TextInput } from '@/components/ui/TextInput'
import { SectionCard, SectionHeader, SettingsButton } from './fields'

type Props = {
  settings: RuntimeSettings
  patch: (changes: Partial<RuntimeSettings>) => void
  save: (fields: Partial<RuntimeSettings>) => Promise<void>
}

// The streaming services the "Skip streaming services" toggle already skips
// (in-app or in a browser tab). Display-only here - the real matching lives in
// the Rust `drm_detector` (STREAMING_APPS / STREAMING_DOMAINS); this list only
// mirrors the human names for the "already skipped" note. Keep in sync with the
// toggle's own description copy in CaptureSection.tsx.
const STREAMING_SERVICES = [
  'Netflix', 'Disney+', 'Hulu', 'Prime Video', 'Apple TV',
  'Peacock', 'Paramount+', 'HBO Max', 'Crunchyroll',
]

/** Reduce a user-entered website to a bare, lowercase domain: drop scheme, path,
 *  and a leading "www.". Matches the host-only matching the Rust side does, so
 *  what the user sees stored is what actually gets matched. */
function normalizeDomain(raw: string): string {
  let s = raw.trim().toLowerCase()
  if (!s) return ''
  const scheme = s.indexOf('://')
  if (scheme !== -1) s = s.slice(scheme + 3)
  s = s.split(/[/?#]/)[0] // authority only
  const at = s.lastIndexOf('@')
  if (at !== -1) s = s.slice(at + 1) // strip userinfo
  s = s.split(':')[0] // strip port
  return s.replace(/^www\./, '')
}

/** A pill. `icon` renders before the label (an app logo); `onClick` makes the
 *  whole chip a tap target (the picker); `onRemove` adds a × (an active entry).
 *  `readOnly` renders it muted with no affordances (the "already skipped" note). */
function Chip({ label, icon, onRemove, onClick, readOnly }: {
  label: string
  icon?: React.ReactNode
  onRemove?: () => void
  onClick?: () => void
  readOnly?: boolean
}) {
  const accent = !readOnly
  return (
    <span
      className="inline-flex items-center gap-1.5 rounded-full text-[12px] whitespace-nowrap"
      style={{
        padding: icon ? '3px 10px 3px 5px' : '4px 10px',
        color: readOnly ? 'var(--t-faint)' : 'var(--t-title)',
        background: accent ? 'color-mix(in srgb, var(--color-state-proposal) 12%, transparent)' : 'transparent',
        border: `1px solid ${accent ? 'color-mix(in srgb, var(--color-state-proposal) 28%, transparent)' : 'var(--t-card-border)'}`,
        cursor: onClick ? 'pointer' : 'default',
      }}
      onClick={onClick}
      role={onClick ? 'button' : undefined}
    >
      {icon}
      <span className="truncate" style={{ maxWidth: 220 }}>{label}</span>
      {onRemove && (
        <button
          type="button"
          aria-label={`Stop ignoring ${label}`}
          onClick={(e) => { e.stopPropagation(); onRemove() }}
          className="leading-none"
          style={{ color: 'var(--t-faint)', fontSize: 14, marginLeft: 1 }}
        >
          ×
        </button>
      )}
    </span>
  )
}

export function CaptureIgnoreCards({ settings, patch, save }: Props) {
  const ignoredApps = settings.ignored_apps ?? []
  const ignoredUrls = settings.ignored_urls ?? []

  const [recent, setRecent] = useState<CaptureApp[]>([])
  const [site, setSite] = useState('')

  useEffect(() => {
    load<CaptureApp[]>('/api/capture-apps', 'get_recent_capture_apps')
      .then(setRecent)
      .catch(() => setRecent([]))
  }, [])

  // Optimistic patch + persist. update_settings refreshes the live capture
  // filter, so the change is enforced on the next frame.
  function commit(fields: Partial<RuntimeSettings>) {
    patch(fields)
    save(fields).catch(() => {})
  }

  function ignoreApp(app: string) {
    if (ignoredApps.some(a => a.toLowerCase() === app.toLowerCase())) return
    commit({ ignored_apps: [...ignoredApps, app] })
  }
  function unignoreApp(app: string) {
    commit({ ignored_apps: ignoredApps.filter(a => a !== app) })
  }
  function ignoreSite() {
    const d = normalizeDomain(site)
    if (!d || ignoredUrls.some(u => u.toLowerCase() === d)) { setSite(''); return }
    commit({ ignored_urls: [...ignoredUrls, d] })
    setSite('')
  }
  function unignoreSite(url: string) {
    commit({ ignored_urls: ignoredUrls.filter(u => u !== url) })
  }

  // Apps seen recently that aren't already ignored — the one-tap picker.
  const ignoredLower = new Set(ignoredApps.map(a => a.toLowerCase()))
  const suggestions = recent.filter(r => !ignoredLower.has(r.app.toLowerCase())).slice(0, 14)

  return (
    <>
      <SectionCard>
        <SectionHeader>Ignored apps</SectionHeader>
        <p className="text-[11px]" style={{ color: 'var(--t-faint)', marginTop: -4 }}>
          Meridian will not watch these apps - their activity is dropped before it is saved, so it
          never appears on your timeline. This applies from now on; anything already recorded stays.
        </p>

        {ignoredApps.length > 0 && (
          <div className="flex flex-wrap gap-2">
            {ignoredApps.map(app => (
              <Chip key={app} label={app} icon={<AppGlyph app={app} size={16} />} onRemove={() => unignoreApp(app)} />
            ))}
          </div>
        )}

        {suggestions.length > 0 && (
          <div className="flex flex-col gap-2">
            <p className="mt-label" style={{ color: 'var(--t-faint)' }}>
              {ignoredApps.length > 0 ? 'Ignore another app you have used' : 'Tap an app you have used to ignore it'}
            </p>
            <div className="flex flex-wrap gap-2">
              {suggestions.map(s => (
                <Chip key={s.app} label={s.app} icon={<AppGlyph app={s.app} size={16} />} onClick={() => ignoreApp(s.app)} />
              ))}
            </div>
          </div>
        )}
        {suggestions.length === 0 && ignoredApps.length === 0 && (
          <p className="text-[11px]" style={{ color: 'var(--t-faint)' }}>
            No captured apps yet - once Meridian has watched for a while, your apps show up here to ignore in one tap.
          </p>
        )}

        <p className="text-[11px]" style={{ color: 'var(--t-faint)', paddingTop: 2 }}>
          Password managers and Meridian&apos;s own windows are always skipped, whether or not they are listed here.
        </p>
      </SectionCard>

      <SectionCard>
        <SectionHeader>Ignored websites</SectionHeader>
        <p className="text-[11px]" style={{ color: 'var(--t-faint)', marginTop: -4 }}>
          Meridian will not watch these sites in your browser. Enter a domain like youtube.com - it
          also covers its subdomains. Everything else in the browser is still captured.
        </p>

        <div className="flex items-center gap-2">
          <TextInput
            value={site}
            onChange={setSite}
            placeholder="youtube.com"
            onKeyDown={(e) => { if (e.key === 'Enter') ignoreSite() }}
          />
          <SettingsButton onClick={ignoreSite} variant="outline">Ignore</SettingsButton>
        </div>

        {ignoredUrls.length > 0 && (
          <div className="flex flex-wrap gap-2">
            {ignoredUrls.map(url => (
              <Chip key={url} label={url} onRemove={() => unignoreSite(url)} />
            ))}
          </div>
        )}

        {settings.pause_on_streaming_video && (
          <div className="flex flex-col gap-2" style={{ paddingTop: 2 }}>
            <p className="mt-label" style={{ color: 'var(--t-faint)' }}>
              Already skipped by &quot;Skip streaming services&quot; above
            </p>
            <div className="flex flex-wrap gap-2">
              {STREAMING_SERVICES.map(s => (
                <Chip key={s} label={s} readOnly />
              ))}
            </div>
          </div>
        )}
      </SectionCard>
    </>
  )
}
