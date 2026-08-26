//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

import { Component } from 'react'
import type { ReactNode } from 'react'
import { isLikelyClerkNetworkError } from '@/lib/clerkNetworkError'
import { Btn } from '../atoms'

type Props = {
  children: ReactNode
  /** Re-mounts the gated subtree with a fresh `initClerk()` call - see
   *  `ClerkGate`, which passes a `key`-bumping callback so this boundary's own
   *  `failed` state resets along with it. */
  onRetry: () => void
  /** Reported so `ClerkGate` can schedule an automatic retry (backoff timer +
   *  an `online` listener) when the failure looks like "no network yet"
   *  rather than a real misconfiguration - that orchestration needs to
   *  survive across remounts of this boundary, so it lives one level up. */
  onError: (error: unknown) => void
}

/** Catches `initClerk()` rejecting inside `ClerkGate`'s `use()` call and shows a
 *  message instead of leaving Suspense's child throw uncaught, which would
 *  otherwise blank the sign-in surface. `use()`/Suspense need a real error
 *  boundary; there is no hook equivalent, hence the class component.
 *
 *  NOTE the "no key at all" case does NOT reach here: `sign_in_required`
 *  (`commands::account`) reports false for a debug build with no key, and the
 *  gates skip Clerk entirely rather than mounting a `ClerkGate` that is certain
 *  to fail. So what lands here is either a key that IS configured and still
 *  didn't init, or a transient network failure - see `isLikelyClerkNetworkError`
 *  for how those two are told apart, and why they need different copy: a real
 *  misconfiguration needs a rebuild, but a login-item launch that raced the
 *  OS's own network bring-up just needs a moment (v1.90.0 staging report:
 *  `docs/vision.md`-adjacent - this is the exact "worked after I reconnected"
 *  case, not a bad key). */
export class ClerkErrorBoundary extends Component<Props, { failed: boolean; networkIssue: boolean }> {
  state: { failed: boolean; networkIssue: boolean } = { failed: false, networkIssue: false }

  static getDerivedStateFromError(error: unknown) {
    return { failed: true, networkIssue: isLikelyClerkNetworkError(error) }
  }

  componentDidCatch(error: unknown) {
    // eslint-disable-next-line no-console -- surfaced nowhere else; this is a dev/misconfiguration signal
    console.error('setup: Clerk sign-in unavailable', error)
    this.props.onError(error)
  }

  render() {
    if (this.state.failed) {
      if (this.state.networkIssue) {
        return (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 8, alignItems: 'flex-start' }}>
            <p style={{ fontSize: 12.5, color: 'var(--t-muted)' }}>
              Sign-in couldn&apos;t reach the network. Meridian will retry automatically once
              you&apos;re back online.
            </p>
            <Btn variant="secondary" size="sm" onClick={this.props.onRetry}>Retry now</Btn>
          </div>
        )
      }
      return (
        <p style={{ fontSize: 12.5, color: 'var(--t-muted)' }}>
          Sign-in couldn&apos;t start - check CLERK_PUBLISHABLE_KEY and restart.
        </p>
      )
    }
    return this.props.children
  }
}
