//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// Settings → Account's account control — see `../signin.tsx` for why this
// module (and everything it imports) is only ever loaded through a
// `next/dynamic(..., { ssr: false })` boundary, never imported directly.
//
// There is no session to sign out of (see RequireEmailCapture.tsx's module
// doc), so unlike the old Clerk-backed version this offers "Change email"
// instead of "Sign out" — a plain overwrite via `save_account_email`, not a
// clear-then-recapture. `onSignedOut` was dropped from this component's props
// entirely along with the branch that used it (see AccountSection.tsx, its
// only caller).

import { useEffect, useState } from 'react'
import { invoke } from '@/lib/bridge'
import { Btn, Spinner } from '../atoms'
import { GateLoading, AccountIdentityRow } from './identity'
import { OtpForm } from './OtpForm'

const SIGNED_IN_CAPTION = 'Verified by a one-time email code'

/** An identity row (avatar, email, how it was captured) with a Change email
 *  action, or the capture form if nothing has ever been saved — the standard
 *  "who am I signed in as" pattern (Linear/Notion/Raycast account rows).
 *  Unlike the setup wizard's step body, this surfaces the captured state
 *  instead of hiding it: the user came here specifically to check/change it. */
function AccountStatus({ onSignedIn }: { onSignedIn: (email: string) => void }) {
  const [email, setEmail] = useState<string | null | undefined>(undefined)
  const [changing, setChanging] = useState(false)

  useEffect(() => {
    let live = true
    invoke<string | null>('get_account_email')
      .then((e) => { if (live) setEmail(e) })
      .catch(() => { if (live) setEmail(null) })
    return () => { live = false }
  }, [])

  if (email === undefined) return <GateLoading />

  if (email && !changing) {
    return (
      <AccountIdentityRow
        email={email}
        caption={SIGNED_IN_CAPTION}
        action={
          <Btn variant="secondary" size="sm" onClick={() => setChanging(true)}>
            Change email
          </Btn>
        }
      />
    )
  }

  return (
    <div className="flex flex-col" style={{ gap: 10 }}>
      {!email && (
        <div>
          <p className="mt-body-sm font-medium" style={{ color: 'var(--t-title)' }}>You&apos;re not signed in</p>
          <p style={{ fontSize: 11, color: 'var(--t-faint)', marginTop: 2 }}>
            Sign in to link this install to your account - we&apos;ll email you a one-time code, no password needed.
          </p>
        </div>
      )}
      <OtpForm
        onSignedIn={(newEmail) => {
          setEmail(newEmail)
          setChanging(false)
          onSignedIn(newEmail)
        }}
      />
      {email && changing && (
        <Btn variant="ghost" size="sm" onClick={() => setChanging(false)} style={{ alignSelf: 'flex-start' }}>
          Cancel
        </Btn>
      )}
    </div>
  )
}

/** Settings → Account's account control. */
export function AccountAuthControl({ onSignedIn }: {
  onSignedIn: (email: string) => void
}) {
  return <AccountStatus onSignedIn={onSignedIn} />
}
