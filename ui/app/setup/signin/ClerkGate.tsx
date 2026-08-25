//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// Shared Clerk bootstrap for both sign-in surfaces (the setup wizard's
// SignInWidget and Settings' AccountAuthControl): resolves
// tauri-plugin-clerk's init promise via Suspense, hands the live instance to
// ClerkProvider, and degrades gracefully outside Tauri or if init rejects.
// `notInTauriMessage` and `fallback` differ per call site; everything else
// about the bootstrap is identical, so it lives here once instead of being
// duplicated across both widgets.

import { Suspense, use, useCallback, useEffect, useRef, useState } from 'react'
import type { ReactNode } from 'react'
import { ClerkProvider } from '@clerk/react'
// eslint-disable-next-line @typescript-eslint/no-var-requires -- no type defs shipped for this community plugin
import { initClerk } from 'tauri-plugin-clerk'
import { isTauri } from '@/lib/bridge'
import { isLikelyClerkNetworkError } from '@/lib/clerkNetworkError'
import { ClerkErrorBoundary } from './ClerkErrorBoundary'

// The initialised clerk instance's type isn't exported by this community
// plugin's (untyped) JS package, so `any` is the only option here.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
function ClerkResolve({ clerkPromise, children }: {
  clerkPromise: Promise<any>
  children: (clerk: any) => ReactNode
}) {
  const clerk = use(clerkPromise)
  return (
    <ClerkProvider publishableKey={clerk.publishableKey} Clerk={clerk}>
      {children(clerk)}
    </ClerkProvider>
  )
}

const INITIAL_RETRY_MS = 5_000
const MAX_RETRY_MS = 60_000

// eslint-disable-next-line @typescript-eslint/no-explicit-any -- see ClerkResolve
type Attempt = { key: number; clerkPromise: Promise<any> | null }

function freshAttempt(key: number): Attempt {
  return { key, clerkPromise: isTauri() ? initClerk() : null }
}

export function ClerkGate({ notInTauriMessage, fallback, children }: {
  notInTauriMessage: string
  fallback: ReactNode
  // eslint-disable-next-line @typescript-eslint/no-explicit-any -- see ClerkResolve
  children: (clerk: any) => ReactNode
}) {
  // `retry()` replaces BOTH the promise `use()` resolves and the boundary's
  // `key` in one update, so a retry always means a genuinely new
  // `initClerk()` call feeding a freshly-mounted (failed: false) boundary -
  // not the same already-rejected promise re-thrown forever.
  const [attempt, setAttempt] = useState<Attempt>(() => freshAttempt(0))
  const retry = useCallback(() => setAttempt((prev) => freshAttempt(prev.key + 1)), [])

  const backoffMs = useRef(INITIAL_RETRY_MS)
  const pending = useRef<{ timer: ReturnType<typeof setTimeout> | null; onlineListener: (() => void) | null }>({
    timer: null,
    onlineListener: null,
  })

  const clearPending = useCallback(() => {
    if (pending.current.timer) clearTimeout(pending.current.timer)
    if (pending.current.onlineListener) window.removeEventListener('online', pending.current.onlineListener)
    pending.current = { timer: null, onlineListener: null }
  }, [])

  // Only a NETWORK-looking failure gets auto-retried - a bad key won't fix
  // itself on a timer, so that case is left to the boundary's static message
  // (see ClerkErrorBoundary's doc). Two independent triggers race to whichever
  // fires first: the browser's `online` event (instant once connectivity is
  // back) and a backoff timer (a floor for browsers/WKWebViews that don't fire
  // `online` reliably in a login-item's background window).
  const handleError = useCallback((error: unknown) => {
    if (!isLikelyClerkNetworkError(error)) return
    clearPending()
    const onlineListener = () => { clearPending(); retry() }
    pending.current.onlineListener = onlineListener
    window.addEventListener('online', onlineListener)
    pending.current.timer = setTimeout(() => { clearPending(); retry() }, backoffMs.current)
    backoffMs.current = Math.min(backoffMs.current * 2, MAX_RETRY_MS)
  }, [clearPending, retry])

  const manualRetry = useCallback(() => { clearPending(); retry() }, [clearPending, retry])

  useEffect(() => clearPending, [clearPending])

  if (!attempt.clerkPromise) {
    return <p style={{ fontSize: 12.5, color: 'var(--t-muted)' }}>{notInTauriMessage}</p>
  }
  return (
    <ClerkErrorBoundary key={attempt.key} onRetry={manualRetry} onError={handleError}>
      <Suspense fallback={fallback}>
        <ClerkResolve clerkPromise={attempt.clerkPromise}>{children}</ClerkResolve>
      </Suspense>
    </ClerkErrorBoundary>
  )
}
