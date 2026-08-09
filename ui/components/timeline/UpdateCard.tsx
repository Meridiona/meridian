//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// DMG auto-update card for the dashboard sidebar (OverviewPanel) - the sibling
// of the tray popover's `.upd` banner, reusing the exact same commands so both
// surfaces behave identically: `check_update` on mount to decide whether to
// show, `install_update` on click (download + verify + relaunch), and the
// `update-progress` event for the live percentage. Consent-based: it only
// appears when a newer version is available and does nothing until clicked.
//
// Styled to match OverviewPanel's "Board cleanup available" CTA (small accent
// card, not a full-width header banner) so it reads as one more sidebar action.
// Outside Tauri (next dev in a plain browser) `invoke` throws, the effect
// swallows it, and the card simply never shows.

'use client'

import { useEffect, useState } from 'react'
import { invoke, isTauri, subscribe } from '@/lib/bridge'
import type { UpdateError, UpdateProgress, UpdateStatus } from '@/lib/api-types'

/**
 * True when `install_update` was refused because an install is ALREADY running
 * (the Rust single-flight guard), rather than because one failed.
 *
 * The card and the tray popover can both be open, and both drive the same
 * command - so the second one clicked always loses this race. Treating that
 * loss as a failure is what showed "Update failed / Click to try again" over a
 * download that went on to succeed.
 *
 * Anything that isn't the exact `{ kind: 'inProgress' }` object gets the safe
 * reading - a failure. An IPC-level throw arrives as an `Error`, and guessing
 * "in progress" there would hide a genuinely broken updater behind a reassuring
 * label. Exported for `ui/__tests__/update-in-progress.test.ts`.
 */
export function isInstallInProgress(err: unknown): boolean {
  return (
    typeof err === 'object' &&
    err !== null &&
    (err as Partial<UpdateError>).kind === 'inProgress'
  )
}

export function UpdateCard() {
  const [status, setStatus] = useState<UpdateStatus | null>(null)
  const [installing, setInstalling] = useState(false)
  const [pct, setPct] = useState<number | null>(null)
  const [failed, setFailed] = useState(false)
  // Set when *another* surface owns the running install. Stays in the installing
  // state (so the shared `update-progress` events keep painting this card) but
  // labels itself honestly until the first percentage lands.
  const [elsewhere, setElsewhere] = useState(false)

  // Check once on mount; only keep an *available* result (up-to-date / error /
  // unsupported stay silent, same as the popover).
  useEffect(() => {
    if (!isTauri()) return
    let cancelled = false
    invoke<UpdateStatus>('check_update')
      .then((s) => {
        if (!cancelled && s && s.state === 'available' && s.version) setStatus(s)
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [])

  // Live download progress (a delta stream, so no snapshot prime -> command null).
  useEffect(() => {
    if (!isTauri()) return
    return subscribe<UpdateProgress>('/api/update/progress', null, 'update-progress', (d) => {
      if (d?.contentLength) setPct(Math.round((d.downloaded / d.contentLength) * 100))
    })
  }, [])

  if (!status) return null

  const onInstall = () => {
    if (installing) return
    setInstalling(true)
    setFailed(false)
    setElsewhere(false)
    setPct(null)
    // Resolves only on failure - success re-execs the app (the relaunch).
    invoke('install_update').catch((err) => {
      if (isInstallInProgress(err)) {
        // The other surface got there first. Stay installing: its progress
        // events are broadcast to every window, so this card keeps tracking the
        // real download and the app relaunches under both of them.
        setElsewhere(true)
        return
      }
      setInstalling(false)
      setFailed(true)
    })
  }

  const title = failed
    ? 'Update failed'
    : installing
      ? 'Updating Meridian'
      : status.mandatory
        ? 'Required update available'
        : 'Update available'

  const subtitle = failed
    ? 'Click to try again'
    : installing
      ? pct !== null
        ? `Downloading... ${pct}%`
        : elsewhere
          ? 'Already in progress...'
          : 'Starting...'
      : `v${status.currentVersion} → v${status.version}`

  return (
    <button
      onClick={onInstall}
      disabled={installing}
      className="w-full text-left rounded-xl px-4 py-3 flex items-center gap-2.5"
      style={{
        background: 'color-mix(in srgb, var(--accent) 12%, transparent)',
        border: '1px solid color-mix(in srgb, var(--accent) 30%, transparent)',
        cursor: installing ? 'default' : 'pointer',
      }}
    >
      <span
        aria-hidden="true"
        className="inline-flex items-center justify-center rounded-full shrink-0 text-[13px] font-bold"
        style={{
          width: 26,
          height: 26,
          background: 'color-mix(in srgb, var(--accent) 20%, transparent)',
          color: 'var(--accent)',
        }}
      >
        ↑
      </span>
      <span className="flex-1 min-w-0">
        <p className="mt-card-title" style={{ color: 'var(--accent)' }}>
          {title}
        </p>
        <p className="mt-body-sm mt-0.5" style={{ color: 'var(--t-muted)' }}>
          {subtitle}
        </p>
      </span>
      {!installing && <span style={{ color: 'var(--accent)' }}>→</span>}
    </button>
  )
}
