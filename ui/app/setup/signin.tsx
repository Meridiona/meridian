//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// Thin `next/dynamic` boundary in front of `./signin/` (the OTP email-capture
// widgets). `{ ssr: false }` keeps this module's importers (`steps.tsx`,
// `AccountSection.tsx`) free of any heavy client-only dependency the `signin/`
// module might ever pick up (e.g. a Turnstile script tag) — the same
// contract this boundary held when the module underneath was Clerk-backed
// (`@clerk/clerk-js` pulled in a transitive Solana wallet-adapter chain that
// touched `window` at module-eval time, crashing the static export's Node
// prerender pass; nothing in the OTP flow does that today, but the boundary
// costs nothing to keep).

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

const OtpFormImpl = dynamic(() => import('./signin/OtpForm').then((m) => m.OtpForm), {
  ssr: false,
  loading: GateLoading,
})

const AccountAuthControlImpl = dynamic(() => import('./signin/AccountAuthControl').then((m) => m.AccountAuthControl), {
  ssr: false,
  loading: GateLoading,
})

const RequireEmailCaptureImpl = dynamic(() => import('./signin/RequireEmailCapture').then((m) => m.RequireEmailCapture), {
  ssr: false,
  // A not-yet-captured user must see the capture screen before the app, so
  // the pre-mount placeholder is a blank surface, not a spinner over app chrome.
  loading: () => null,
})

/** The setup wizard's Email step body. Calls `onSignedIn(email)` once a code
 *  verifies (fresh capture, or `onDevBypass` when no Worker is configured) —
 *  never gates or hides anything itself; the wizard's `SignInBody`
 *  (steps.tsx) decides what to render based on that callback's result
 *  (`wiz.signedInEmail`). */
export function OtpForm({ onSignedIn, onDevBypass }: {
  onSignedIn: (email: string) => void
  onDevBypass?: () => void
}) {
  return <OtpFormImpl onSignedIn={onSignedIn} onDevBypass={onDevBypass} />
}

/** Settings → Account's account control (see AccountSection.tsx) — shows the
 *  captured email and lets it be changed. See `./signin/AccountAuthControl.tsx`. */
export function AccountAuthControl({ onSignedIn }: {
  onSignedIn: (email: string) => void
}) {
  return <AccountAuthControlImpl onSignedIn={onSignedIn} />
}

/** Product-wide email-capture gate for the dashboard window — renders
 *  `children` once an email has ever been captured, otherwise an inline
 *  full-window capture screen. A fire-once check, not a live session: there
 *  is no sign-out to re-lock against. See `./signin/RequireEmailCapture.tsx`. */
export function RequireEmailCapture({ children }: { children: ReactNode }) {
  return <RequireEmailCaptureImpl>{children}</RequireEmailCaptureImpl>
}
