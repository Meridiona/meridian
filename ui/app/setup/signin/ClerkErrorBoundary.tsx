//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

import { Component } from 'react'
import type { ReactNode } from 'react'

/** Catches `initClerk()` rejecting inside `ClerkGate`'s `use()` call — e.g.
 *  the Rust side never registered the Clerk plugin (no publishable key
 *  configured on this build; see `lib.rs`'s conditional registration) — and
 *  shows a message instead of leaving Suspense's child throw uncaught, which
 *  would otherwise blank the sign-in surface. `use()`/Suspense need a real
 *  error boundary; there is no hook equivalent, hence the class component. */
export class ClerkErrorBoundary extends Component<{ children: ReactNode }, { failed: boolean }> {
  state: { failed: boolean } = { failed: false }

  static getDerivedStateFromError() {
    return { failed: true }
  }

  componentDidCatch(error: unknown) {
    // eslint-disable-next-line no-console -- surfaced nowhere else; this is a dev/misconfiguration signal
    console.error('setup: Clerk sign-in unavailable', error)
  }

  render() {
    if (this.state.failed) {
      return (
        <p style={{ fontSize: 12.5, color: 'var(--t-muted)' }}>
          Sign-in isn&apos;t configured for this build - set CLERK_PUBLISHABLE_KEY and restart.
        </p>
      )
    }
    return this.props.children
  }
}
