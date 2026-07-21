//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// Product-wide sign-in gate for the dashboard window. Unlike the setup
// wizard's SignInWidget (which reports a session up and lets the wizard decide
// what to show) and Settings' AccountAuthControl (which surfaces who's signed
// in), this HIDES the whole app until a live Clerk session exists: signed out
// => an inline full-window sign-in screen, signed in => the real UI. Sign-out
// (Settings -> Account) tears down the Clerk session, `useUser().isSignedIn`
// flips false, and this re-locks immediately.
//
// Loaded only through the `next/dynamic({ ssr: false })` boundary in
// `../signin.tsx` (as `RequireSignIn`) — never imported directly — for the
// same static-export reason documented there.

import type { ReactNode } from 'react'
import { invoke } from '@/lib/bridge'
import { useSignedInEmail } from './useSignedInEmail'
import { ClerkGate } from './ClerkGate'
import { EmailCodeForm } from './EmailCodeForm'

/** Full-window loading/placeholder — fills the whole app surface (not the
 *  120px inline box the wizard/Settings use) so the gate never flashes a small
 *  centred spinner over an empty page. */
function FullScreenCenter({ children }: { children: ReactNode }) {
  return (
    <div
      className="flex flex-col items-center justify-center h-[100svh] overflow-hidden"
      style={{ background: 'var(--win-bg)', padding: 24 }}
    >
      {children}
    </div>
  )
}

/** The inline sign-in screen a signed-out user sees instead of the app. Reuses
 *  the same Clerk email one-time-code form as the wizard and Settings, wrapped
 *  in a titled card so it reads as a deliberate lock, not a stray form. */
function LockedSignInScreen() {
  return (
    <FullScreenCenter>
      <div style={{ width: '100%', maxWidth: 380 }}>
        <div style={{ textAlign: 'center', marginBottom: 22 }}>
          <h1 className="mt-title-lg" style={{ color: 'var(--t-title)' }}>Sign in to Meridian</h1>
          <p className="mt-body-sm mt-2" style={{ color: 'var(--t-muted)' }}>
            Sign in to use Meridian - we&apos;ll email you a one-time code, no password needed.
          </p>
        </div>
        <EmailCodeForm />
      </div>
    </FullScreenCenter>
  )
}

/** Renders `children` only once Clerk reports a live session; otherwise the
 *  inline sign-in screen. Persists the email Rust-side on sign-in (the mirror
 *  Settings/analytics read) via `save_account_email`. */
function Gate({ children }: { children: ReactNode }) {
  const { isLoaded, isSignedIn } = useSignedInEmail((email) => {
    invoke('save_account_email', { email }).catch(() => {})
  })
  // Still resolving Clerk's own async session check — hold a blank surface
  // rather than flashing the sign-in form to an already-signed-in user.
  if (!isLoaded) return <FullScreenCenter>{null}</FullScreenCenter>
  if (!isSignedIn) return <LockedSignInScreen />
  return <>{children}</>
}

/** Wrap the dashboard app in this to make sign-in compulsory. Outside the
 *  Tauri webview (a plain browser preview of the static export) there's no
 *  `tauri-plugin-clerk` bridge, so it degrades to a notice rather than
 *  hanging — the app only ever runs inside Tauri in practice. */
export function RequireSignIn({ children }: { children: ReactNode }) {
  return (
    <ClerkGate notInTauriMessage="Open Meridian to sign in." fallback={<FullScreenCenter>{null}</FullScreenCenter>}>
      {() => <Gate>{children}</Gate>}
    </ClerkGate>
  )
}
