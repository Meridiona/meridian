//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// DMG auto-update banner for the dashboard - the sibling of the tray popover's
// `.upd` banner (tray/src/index.html + app.js), reusing the exact same commands
// so both surfaces behave identically: `check_update` on mount to decide
// whether to show, `install_update` on click (download + verify + relaunch),
// and the `update-progress` event for the live percentage. Consent-based: it
// only appears when a newer version is available and does nothing until the
// user clicks. Rendered by LayoutBanners on every non-setup route.
//
// Outside Tauri (next dev in a plain browser) `invoke` throws, the mount effect
// swallows it, and the banner simply never shows - matching the rest of the
// bridge's browser-safe degradation.

'use client'

import { useEffect, useState } from 'react'
import { invoke, isTauri, subscribe } from '@/lib/bridge'
import type { UpdateProgress, UpdateStatus } from '@/lib/api-types'

export default function UpdateBanner() {
  const [status, setStatus] = useState<UpdateStatus | null>(null)
  const [installing, setInstalling] = useState(false)
  const [pct, setPct] = useState<number | null>(null)
  const [failed, setFailed] = useState(false)

  // Check once on mount; only keep an *available* result (up-to-date / error /
  // unsupported stay silent on the dashboard, same as the popover).
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
    // Resolves only on failure - success re-execs the app (the relaunch).
    invoke('install_update').catch(() => {
      setInstalling(false)
      setFailed(true)
    })
  }

  const label = failed
    ? 'Update failed - try again'
    : installing
      ? pct !== null
        ? `Downloading... ${pct}%`
        : 'Starting...'
      : status.mandatory
        ? 'Required update available'
        : 'Update available'

  return (
    <div style={{ position: 'sticky', top: 0, zIndex: 49 }}>
      <button
        type="button"
        onClick={onInstall}
        disabled={installing}
        style={{
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
          gap: 12,
          width: '100%',
          textAlign: 'left',
          padding: '10px 16px',
          background: 'var(--accent-soft)',
          borderBottom: '1px solid var(--accent)',
          cursor: installing ? 'default' : 'pointer',
        }}
      >
        <span style={{ display: 'flex', alignItems: 'center', gap: 10, minWidth: 0 }}>
          <span
            aria-hidden="true"
            style={{
              display: 'inline-flex',
              alignItems: 'center',
              justifyContent: 'center',
              flexShrink: 0,
              width: 18,
              height: 18,
              borderRadius: 6,
              fontSize: 12,
              fontWeight: 700,
              color: '#fff',
              background: 'var(--accent)',
            }}
          >
            ↑
          </span>
          <span style={{ fontSize: 12.5, fontWeight: 600, color: 'var(--t-title)', whiteSpace: 'nowrap' }}>
            {label}
          </span>
          <span
            style={{
              fontFamily: 'var(--font-mono)',
              fontSize: 11,
              color: 'var(--t-muted)',
              flexShrink: 0,
              whiteSpace: 'nowrap',
            }}
          >
            v{status.currentVersion} → v{status.version}
          </span>
        </span>
        {!installing && (
          <span
            style={{
              flexShrink: 0,
              fontSize: 11,
              fontWeight: 700,
              color: '#fff',
              background: 'var(--accent)',
              borderRadius: 6,
              padding: '4px 10px',
              whiteSpace: 'nowrap',
            }}
          >
            Restart & Update
          </span>
        )}
      </button>
    </div>
  )
}
