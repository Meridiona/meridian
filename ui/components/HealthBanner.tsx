// meridian — normalises screenpipe activity into structured app sessions
'use client'

import { useEffect, useState } from 'react'

interface HealthStatus {
  a11y_helper_trusted: boolean
  error?: string
}

export default function HealthBanner() {
  const [health, setHealth] = useState<HealthStatus | null>(null)
  const [dismissed, setDismissed] = useState(false)

  useEffect(() => {
    // Fetch health status on mount and every 30 seconds
    const fetchHealth = async () => {
      try {
        const res = await fetch('/api/health')
        const data = await res.json()
        setHealth(data)
      } catch (e) {
        // Silently fail if health check unavailable
      }
    }

    fetchHealth()
    const interval = setInterval(fetchHealth, 30000)

    return () => clearInterval(interval)
  }, [])

  // Show banner only if a11y-helper is not trusted and not dismissed
  if (!health || health.a11y_helper_trusted || dismissed) {
    return null
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
