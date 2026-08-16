//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

import { Component } from 'react'
import type { ReactNode } from 'react'

/** Catches `initClerk()` rejecting inside `ClerkGate`'s `use()` call and shows a
 *  message instead of leaving Suspense's child throw uncaught, which would
 *  otherwise blank the sign-in surface. `use()`/Suspense need a real error
 *  boundary; there is no hook equivalent, hence the class component.
 *
 *  NOTE the "no key at all" case does NOT reach here: `sign_in_required`
 *  (`commands::account`) reports false for a debug build with no key, and the
 *  gates skip Clerk entirely rather than mounting a `ClerkGate` that is certain
 *  to fail. So what lands here is a key that IS configured and still didn't
 *  init — malformed, or the wrong instance — which is why the copy points at
 *  the value being bad rather than missing. */
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
          Sign-in couldn&apos;t start - check CLERK_PUBLISHABLE_KEY and restart.
        </p>
      )
    }
    return this.props.children
  }
}
