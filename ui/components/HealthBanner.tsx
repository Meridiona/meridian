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
  // Dismissal is keyed to the SIGNATURE of the banner that was dismissed, not a global boolean,
  // so dismissing one problem can't suppress a different (or worse) one that arises later - e.g.
  // dismissing the a11y notice must not hide a subsequent "your provider is unavailable, summaries
  // paused" banner. When the active problem's signature changes, the old dismissal no longer
  // matches and the banner reappears.
  const [dismissedSig, setDismissedSig] = useState<string | null>(null)

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
  // A signature that identifies the active (highest-priority) banner AND its content, so a
  // dismissal is scoped to exactly that alert. `null` when there's nothing to show.
  const pName = health?.llm_provider_name ?? ''
  const pDetail = health?.llm_provider_detail ?? ''
  const activeSig = showDatabaseError
    ? `db:${health?.error ?? 'schema'}`
    : showProviderError
      ? `provider-unavailable:${pName}:${pDetail}`
      : showProviderRateLimit
        ? `provider-ratelimit:${pName}:${pDetail}`
        : showA11yWarning
          ? 'a11y'
          : null

  // A dismissal only survives while the thing dismissed is still true. Once the
  // problem clears, the dismissal is spent — so if the SAME problem comes back
  // later, it is announced again instead of being silently swallowed by a
  // decision the user made about a different occurrence of it.
  //
  // Without this, "dismiss → fix it → it breaks again the same way" showed no
  // banner at all for the rest of the session, because the signature is content-
  // derived and a recurrence reproduces it byte for byte. The failure mode is
  // invisible from here: the app looks healthy precisely when it is not.
  useEffect(() => {
    if (!activeSig) setDismissedSig(null)
  }, [activeSig])

  if (!health || !activeSig || activeSig === dismissedSig) {
    return null
  }

  const dismiss = () => setDismissedSig(activeSig)

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
          onClick={dismiss}
          className="px-3 py-1 text-xs rounded hover:opacity-70 transition-opacity"
          style={{ color: 'var(--ink-3)', border: '1px solid var(--rule)' }}
        >
          Dismiss
        </button>
      </div>
    )
  }

  // CRITICAL, AND NOT DISMISSIBLE — the same treatment as a missing database,
  // because it has the same consequence.
  //
  // This used to be a soft yellow notice with a Dismiss button, describing the
  // problem as an inconvenience to the hourly summaries. It is not: the TIMELINE
  // itself is produced by the hourly fold (`workstream::run`, which calls
  // `llm::complete` unconditionally), so with no working provider Meridian
  // generates nothing at all - no task cards, no summaries, no worklogs. The whole
  // product is inert, and the day being lived right now is the day being lost.
  //
  // So there is nothing to dismiss. A Dismiss button on a blocker offers to hide
  // the only evidence that the app has stopped working, and whoever takes it gets
  // a dashboard that looks idle rather than broken - then finds the hole in their
  // week days later, when the work is no longer reconstructible. It stays up until
  // it is no longer true, which is precisely when the fix has landed.
  if (showProviderError) {
    const providerName = health.llm_provider_name ?? 'Your AI provider'
    return (
      <div
        className="w-full px-6 py-3.5 flex items-center gap-3 border-b"
        style={{ borderBottomColor: 'var(--rule)', backgroundColor: 'rgba(239, 68, 68, 0.08)' }}
      >
        <span className="text-lg">🚨</span>
        <div className="flex-1">
          <p className="text-sm" style={{ color: 'var(--ink-2)' }}>
            <strong>{providerName} isn&apos;t available - Meridian has stopped writing your day</strong>
          </p>
          <p className="text-xs mt-0.5" style={{ color: 'var(--ink-3)' }}>
            {health.llm_provider_detail ? `${health.llm_provider_detail}. ` : ''}
            Your timeline, summaries and worklogs all need a model, so nothing new is being
            recorded until this is fixed. Open Settings → Intelligence to reinstall, sign in,
            or pick another provider.
          </p>
        </div>
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
              You&apos;re signed in and nothing is lost - summaries will catch up on their own once the limit resets.
            </p>
          </div>
        </div>
        <button
          onClick={dismiss}
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
        onClick={dismiss}
        className="px-3 py-1 text-xs rounded hover:opacity-70 transition-opacity"
        style={{ color: 'var(--ink-3)', border: '1px solid var(--rule)' }}
      >
        Dismiss
      </button>
    </div>
  )
}
