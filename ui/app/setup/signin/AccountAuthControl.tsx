//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// Settings → Account's sign-in/sign-out control — see `../signin.tsx` for
// why this module (and everything it imports) is only ever loaded through a
// `next/dynamic(..., { ssr: false })` boundary, never imported directly.
//
// Unlike SignInWidget and RequireSignIn, this does NOT wrap itself in a
// ClerkGate: Settings is only ever reachable from inside the dashboard,
// which RequireSignIn (app/page.tsx) already wraps in one ClerkProvider.
// A second <ClerkProvider> here would nest inside that one, and
// @clerk/react hard-refuses that ("You've added multiple <ClerkProvider>
// components") — the exact crash this used to throw, caught by
// ClerkErrorBoundary and shown as a generic "not configured" message that
// had nothing to do with configuration. useClerk()/useSignedInEmail() below
// read from RequireSignIn's already-live provider instead.

import { useState, useEffect } from 'react'
import { useClerk } from '@clerk/react'
import { invoke } from '@/lib/bridge'
import { Btn, Spinner } from '../atoms'
import { useSignedInEmail } from './useSignedInEmail'
import { GateLoading, AccountIdentityRow } from './identity'
import { EmailCodeForm } from './EmailCodeForm'

const SIGNED_IN_CAPTION = 'Signed in with a one-time email code'

/** An identity row (avatar, email, how they signed in) with a Sign out
 *  action, or the sign-in form if not signed in yet — the standard "who am I
 *  signed in as" pattern (Linear/Notion/Raycast account rows). Unlike the
 *  setup wizard's `SignInWidget`, this surfaces the signed-in state instead
 *  of hiding it: the user came here specifically to check/change it. */
function AccountStatus({ onSignedIn, onSignedOut }: {
  onSignedIn: (email: string) => void
  onSignedOut: () => void
}) {
  const { signOut } = useClerk()
  const [busy, setBusy] = useState(false)
  const [err, setErr] = useState('')
  const { isLoaded, isSignedIn, email, resetNotified } = useSignedInEmail(onSignedIn)

  if (!isLoaded) return <GateLoading />

  if (isSignedIn && email) {
    return (
      <div className="flex flex-col" style={{ gap: 8 }}>
        <AccountIdentityRow
          email={email}
          caption={SIGNED_IN_CAPTION}
          action={
            <Btn
              variant="secondary"
              size="sm"
              disabled={busy}
              onClick={async () => {
                setBusy(true); setErr('')
                try {
                  await signOut()
                  resetNotified()
                  onSignedOut()
                } catch {
                  setErr('Could not sign out - try again.')
                }
                setBusy(false)
              }}
            >
              {busy ? <Spinner size={13} width={1.8} /> : 'Sign out'}
            </Btn>
          }
        />
        {err && <p style={{ fontSize: 11, color: 'var(--color-state-pending)' }}>{err}</p>}
      </div>
    )
  }

  return (
    <div className="flex flex-col" style={{ gap: 10 }}>
      <div>
        <p className="mt-body-sm font-medium" style={{ color: 'var(--t-title)' }}>You&apos;re not signed in</p>
        <p style={{ fontSize: 11, color: 'var(--t-faint)', marginTop: 2 }}>
          Sign in to link this install to your account - we&apos;ll email you a one-time code, no password needed.
        </p>
      </div>
      <EmailCodeForm />
    </div>
  )
}

/** Settings → Account's sign-in/sign-out control. Renders straight into the
 *  dashboard's existing Clerk session (see the module doc above) — Clerk is
 *  already loaded by the time Settings is reachable, so `AccountStatus`'s
 *  own `!isLoaded` check is the only loading state this needs.
 *
 *  EXCEPT when sign-in isn't required at all. `RequireSignIn` skips mounting
 *  Clerk entirely in a debug build with no publishable key
 *  (`commands::account::sign_in_required`), and this component's whole design
 *  is to borrow THAT provider rather than nest a second one — so with the gate
 *  off there is no provider to borrow and `useClerk()` throws
 *  "useClerk can only be used within the <ClerkProvider /> component",
 *  taking the entire Settings modal down with it. The guard below is why this
 *  asks before rendering `AccountStatus`.
 *
 *  Note the check lives HERE and not at the call site: this is the component
 *  that depends on the provider, so any future caller inherits the guard
 *  instead of having to remember it. */
export function AccountAuthControl({ onSignedIn, onSignedOut }: {
  onSignedIn: (email: string) => void
  onSignedOut: () => void
}) {
  const [signInRequired, setSignInRequired] = useState<boolean | null>(null)

  // Defaults to REQUIRED on a failed invoke, matching `RequireSignIn`'s own
  // fail-closed default. The two must agree: if that one gated, a provider is
  // mounted and `AccountStatus` is safe to render.
  useEffect(() => {
    invoke<boolean>('sign_in_required')
      .then(setSignInRequired)
      .catch(() => setSignInRequired(true))
  }, [])

  if (signInRequired === null) return <GateLoading />

  if (!signInRequired) {
    return (
      <div className="flex flex-col" style={{ gap: 4 }}>
        <p className="mt-body-sm font-medium" style={{ color: 'var(--t-title)' }}>Sign-in is off in this build</p>
        <p style={{ fontSize: 11, color: 'var(--t-faint)' }}>
          This is a source build with no Clerk key configured. Set CLERK_PUBLISHABLE_KEY
          in your .env and restart to sign in.
        </p>
      </div>
    )
  }

  return <AccountStatus onSignedIn={onSignedIn} onSignedOut={onSignedOut} />
}
