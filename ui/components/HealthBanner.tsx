//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

import { useEffect, useState } from 'react'
import { subscribe } from '@/lib/bridge'

interface HealthStatus {
  a11y_helper_trusted?: boolean
  database_ready?: boolean
  error?: string
  /** Whether the in-use LLM provider is usable. `false` → the provider is missing or failing,
   *  so summaries are paused/degraded. `llm_provider_name`/`_detail` fill the banner copy. */
  llm_provider_ok?: boolean
  /** `true` → the provider is usable but rate-limited: a softer "catching up" notice, not the
   *  "unavailable" alarm (it clears on its own). Only meaningful when `llm_provider_ok !== false`. */
  llm_provider_rate_limited?: boolean
  llm_provider_name?: string
  llm_provider_detail?: string
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

  // One banner at a time, in priority order: database not ready (critical) > in-use LLM provider
  // unavailable (summaries paused) > provider rate-limited (summaries catching up, soft notice) >
  // a11y-helper not trusted (capture degraded).
  const showDatabaseError = !!health && health.database_ready === false
  const showProviderError = !!health && health.llm_provider_ok === false && !showDatabaseError
  const showProviderRateLimit =
    !!health && health.llm_provider_rate_limited === true && health.llm_provider_ok !== false && !showDatabaseError
  const showA11yWarning =
    !!health && health.a11y_helper_trusted === false && !showDatabaseError && !showProviderError && !showProviderRateLimit
  const anyProblem = showDatabaseError || showProviderError || showProviderRateLimit || showA11yWarning

  if (!health || !anyProblem || dismissed) {
    return null
  }

  if (showDatabaseError) {
    const isNotFound = health.error?.toLowerCase().includes('not found') ?? false
    const bannerTitle = isNotFound ? 'Database not found' : 'Database schema mismatch'
    const defaultDetail = isNotFound
      ? 'Restart Meridian to start the daemon.'
      : <>The database needs migration: <code className="text-xs font-mono">meridian migrate-db</code></>
    return (
      <div
        className="w-full px-6 py-3.5 flex items-center justify-between border-b"
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
              {health.error ?? defaultDetail}
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

  if (showProviderError) {
    const providerName = health.llm_provider_name ?? 'Your AI provider'
    return (
      <div
        className="w-full px-6 py-3.5 flex items-center justify-between border-b"
        style={{ borderBottomColor: 'var(--rule)', backgroundColor: 'rgba(253, 224, 71, 0.08)' }}
      >
        <div className="flex items-center gap-3 flex-1">
          <span className="text-lg">🤖</span>
          <div className="flex-1">
            <p className="text-sm" style={{ color: 'var(--ink-2)' }}>
              <strong>{providerName} isn&apos;t available</strong>
            </p>
            <p className="text-xs mt-0.5" style={{ color: 'var(--ink-3)' }}>
              {health.llm_provider_detail ? `${health.llm_provider_detail}. ` : ''}
              Hourly summaries are paused — open Settings → Intelligence to reinstall, sign in, or pick another provider.
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

  if (showProviderRateLimit) {
    const providerName = health.llm_provider_name ?? 'Your AI provider'
    return (
      <div
        className="w-full px-6 py-3.5 flex items-center justify-between border-b"
        style={{ borderBottomColor: 'var(--rule)', backgroundColor: 'rgba(59, 130, 246, 0.08)' }}
      >
        <div className="flex items-center gap-3 flex-1">
          <span className="text-lg">⏳</span>
          <div className="flex-1">
            <p className="text-sm" style={{ color: 'var(--ink-2)' }}>
              <strong>{providerName} is rate-limited</strong>
            </p>
            <p className="text-xs mt-0.5" style={{ color: 'var(--ink-3)' }}>
              {health.llm_provider_detail ? `${health.llm_provider_detail}. ` : ''}
              You&apos;re signed in and nothing is lost — summaries will catch up on their own once the limit resets.
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
      className="w-full px-6 py-3.5 flex items-center justify-between bg-yellow-50 border-b"
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
