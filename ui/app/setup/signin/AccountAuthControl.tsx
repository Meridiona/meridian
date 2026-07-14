//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// Settings → Account's sign-in/sign-out control — see `../signin.tsx` for
// why this module (and everything it imports) is only ever loaded through a
// `next/dynamic(..., { ssr: false })` boundary, never imported directly.

import { useState } from 'react'
import { useClerk } from '@clerk/react'
import { Btn, Spinner } from '../atoms'
import { useSignedInEmail } from './useSignedInEmail'
import { ClerkGate } from './ClerkGate'
import { GateLoading, AccountIdentityRow } from './identity'
import { EmailCodeForm } from './EmailCodeForm'

const SIGNED_IN_CAPTION = 'Signed in with a one-time email code'

/** Optimistic render for the moment between mount and Clerk finishing its
 *  own (async) session check — used as `ClerkGate`'s Suspense fallback when
 *  the Rust-persisted email (`get_account_email`) is already known, so
 *  Settings → Account shows the identity immediately instead of a bare
 *  spinner on every open, matching how account pages elsewhere avoid a
 *  needless loading flash for state they already have cached. Sign out isn't
 *  wired yet at this point (no live Clerk instance) — a static spinner in
 *  its place, swapped for the real button the moment Clerk resolves. */
function KnownAccountFallback({ email }: { email: string }) {
  return <AccountIdentityRow email={email} caption={SIGNED_IN_CAPTION} action={<Spinner size={13} width={1.8} />} />
}

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

/** Settings → Account's sign-in/sign-out control. `knownEmail` (the
 *  Rust-persisted email, passed by `AccountSection`) renders instantly while
 *  Clerk's own async session check is still in flight — see
 *  `KnownAccountFallback`. */
export function AccountAuthControl({ onSignedIn, onSignedOut, knownEmail }: {
  onSignedIn: (email: string) => void
  onSignedOut: () => void
  knownEmail?: string | null
}) {
  const fallback = knownEmail ? <KnownAccountFallback email={knownEmail} /> : <GateLoading />
  return (
    <ClerkGate notInTauriMessage="Open Meridian to manage sign-in." fallback={fallback}>
      {() => <AccountStatus onSignedIn={onSignedIn} onSignedOut={onSignedOut} />}
    </ClerkGate>
  )
}
