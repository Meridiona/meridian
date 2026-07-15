//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// Shared Clerk bootstrap for both sign-in surfaces (the setup wizard's
// SignInWidget and Settings' AccountAuthControl): resolves
// tauri-plugin-clerk's init promise via Suspense, hands the live instance to
// ClerkProvider, and degrades gracefully outside Tauri or if init rejects.
// `notInTauriMessage` and `fallback` differ per call site; everything else
// about the bootstrap is identical, so it lives here once instead of being
// duplicated across both widgets.

import { Suspense, use, useState } from 'react'
import type { ReactNode } from 'react'
import { ClerkProvider } from '@clerk/react'
// eslint-disable-next-line @typescript-eslint/no-var-requires -- no type defs shipped for this community plugin
import { initClerk } from 'tauri-plugin-clerk'
import { isTauri } from '@/lib/bridge'
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

export function ClerkGate({ notInTauriMessage, fallback, children }: {
  notInTauriMessage: string
  fallback: ReactNode
  // eslint-disable-next-line @typescript-eslint/no-explicit-any -- see ClerkResolve
  children: (clerk: any) => ReactNode
}) {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any -- see ClerkResolve
  const [clerkPromise] = useState<Promise<any> | null>(() => (isTauri() ? initClerk() : null))

  if (!clerkPromise) {
    return <p style={{ fontSize: 12.5, color: 'var(--t-muted)' }}>{notInTauriMessage}</p>
  }
  return (
    <ClerkErrorBoundary>
      <Suspense fallback={fallback}>
        <ClerkResolve clerkPromise={clerkPromise}>{children}</ClerkResolve>
      </Suspense>
    </ClerkErrorBoundary>
  )
}
