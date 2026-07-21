//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// Thin `next/dynamic` boundary in front of `./signin/` (the real Clerk
// sign-in widgets). `{ ssr: false }` is load-bearing here — `@clerk/clerk-js`
// pulls in a transitive Solana wallet-adapter chain that touches `window` at
// module-eval time, which Next's Node prerender pass has none of, so a direct
// import crashes the static export build. Having zero `@clerk/react` imports
// at this top level is exactly what lets `steps.tsx` (and `AccountSection.tsx`)
// import this module directly and safely.

import dynamic from 'next/dynamic'
import type { ReactNode } from 'react'
import { Spinner } from './atoms'

function GateLoading() {
  return (
    <div className="flex flex-col items-center justify-center" style={{ minHeight: 120, gap: 12 }}>
      <Spinner size={22} width={2} />
      <p style={{ fontSize: 12.5, color: 'var(--t-faint)' }}>Loading…</p>
    </div>
  )
}

const SignInWidgetImpl = dynamic(() => import('./signin/SignInWidget').then((m) => m.SignInWidget), {
  ssr: false,
  loading: GateLoading,
})

const AccountAuthControlImpl = dynamic(() => import('./signin/AccountAuthControl').then((m) => m.AccountAuthControl), {
  ssr: false,
  loading: GateLoading,
})

const RequireSignInImpl = dynamic(() => import('./signin/RequireSignIn').then((m) => m.RequireSignIn), {
  ssr: false,
  // A signed-out user must sign in before seeing the app, so the pre-Clerk
  // placeholder is a blank surface, not a spinner over app chrome.
  loading: () => null,
})

/** The setup wizard's Sign-in step body. Calls `onSignedIn(email)` once a
 *  Clerk session exists (fresh sign-in, or an already-persisted one from a
 *  previous launch) — never gates or hides anything itself; the wizard's
 *  `SignInBody` (steps.tsx) decides what to render based on that callback's
 *  result (`wiz.signedInEmail`). */
export function SignInWidget({ onSignedIn }: { onSignedIn: (email: string) => void }) {
  return <SignInWidgetImpl onSignedIn={onSignedIn} />
}

/** Settings → Account's sign-in/sign-out control (see AccountSection.tsx) —
 *  same Clerk session as the setup wizard, but this one shows who's signed
 *  in and lets them sign out, since that's the whole reason to visit it.
 *  `knownEmail` is the Rust-persisted email — passed straight through so the
 *  identity row can render before Clerk's own session check resolves. */
export function AccountAuthControl({ onSignedIn, onSignedOut, knownEmail }: {
  onSignedIn: (email: string) => void
  onSignedOut: () => void
  knownEmail?: string | null
}) {
  return <AccountAuthControlImpl onSignedIn={onSignedIn} onSignedOut={onSignedOut} knownEmail={knownEmail} />
}

/** Product-wide sign-in gate for the dashboard window — renders `children`
 *  only when a live Clerk session exists, otherwise an inline full-window
 *  sign-in screen. Makes sign-in compulsory: sign out and the app re-locks.
 *  See `./signin/RequireSignIn.tsx`. */
export function RequireSignIn({ children }: { children: ReactNode }) {
  return <RequireSignInImpl>{children}</RequireSignInImpl>
}
