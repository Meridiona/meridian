//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

import { useEffect, useState } from 'react'
import { subscribe } from '@/lib/bridge'

interface HealthStatus {
  a11y_helper_trusted?: boolean
  database_ready?: boolean
  error?: string
}

export default function HealthBanner() {
  const [health, setHealth] = useState<HealthStatus | null>(null)
  const [dismissed, setDismissed] = useState(false)

  useEffect(() => {
    // health-update (Tauri event) in the app, /api/health/stream SSE in a browser.
    return subscribe<HealthStatus>('/api/health/stream', 'get_health', 'health-update', (data) => {
      // Ignore empty objects ({}) pushed before any check has data.
      if (data && Object.keys(data).length > 0) setHealth(data)
    })
  }, [])

  // Show banner if database is not ready (critical), or a11y-helper is not trusted, and not dismissed
  const showDatabaseError = health && health.database_ready === false
  const showA11yWarning = health && health.a11y_helper_trusted === false && health.database_ready !== false

  if (!health || (health.a11y_helper_trusted !== false && health.database_ready !== false) || dismissed) {
    return null
  }

  if (showDatabaseError) {
    const isNotFound = health.error?.toLowerCase().includes('not found') ?? false
    const bannerTitle = isNotFound ? 'Database not found' : 'Database schema mismatch'
    const defaultDetail = isNotFound
      ? <>Start the daemon: <code className="text-xs font-mono">launchctl load ~/Library/LaunchAgents/com.meridiona.daemon.plist</code></>
      : <>The database needs migration: <code className="text-xs font-mono">meridian migrate-db</code></>
    return (
      <div
        className="w-full px-4 py-3 flex items-center justify-between border-b"
        style={{
          borderBottomColor: 'var(--rule)',
          backgroundColor: 'rgba(239, 68, 68, 0.08)',
        }}
      >
        <div className="flex items-center gap-3 flex-1">
          <span className="text-lg">🚨</span>
          <div className="flex-1">
            <p className="text-sm" style={{ color: 'var(--ink-2)' }}>
              <strong>{bannerTitle}</strong>
            </p>
            <p className="text-xs mt-0.5" style={{ color: 'var(--ink-3)' }}>
              {isNotFound ? defaultDetail : (health.error ?? defaultDetail)}
            </p>
          </div>
        </div>
        <button
          onClick={() => setDismissed(true)}
          className="px-3 py-1 text-xs rounded hover:opacity-70 transition-opacity"
          style={{ color: 'var(--ink-3)', border: '1px solid var(--rule)' }}
        >
          Dismiss
        </button>
      </div>
    )
  }

  return (
    <div
      className="w-full px-4 py-3 flex items-center justify-between bg-yellow-50 border-b"
      style={{
        borderBottomColor: 'var(--rule)',
        backgroundColor: 'rgba(253, 224, 71, 0.08)',
      }}
    >
      <div className="flex items-center gap-3 flex-1">
        <span className="text-lg">⚠️</span>
        <div className="flex-1">
          <p className="text-sm" style={{ color: 'var(--ink-2)' }}>
            <strong>Electron apps (Claude, Codex, VS Code) are invisible to capture</strong>
          </p>
          <p className="text-xs mt-0.5" style={{ color: 'var(--ink-3)' }}>
            Grant accessibility permission to a11y-helper: <code className="text-xs">System Settings → Privacy &amp; Security → Accessibility → add ~/.meridian/bin/meridian-a11y-helper and toggle it on</code>
          </p>
        </div>
      </div>
      <button
        onClick={() => setDismissed(true)}
        className="px-3 py-1 text-xs rounded hover:opacity-70 transition-opacity"
        style={{ color: 'var(--ink-3)', border: '1px solid var(--rule)' }}
      >
        Dismiss
      </button>
    </div>
  )
}
