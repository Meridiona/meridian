//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// Product-wide email-capture gate for the dashboard window. Replaces
// RequireSignIn.tsx (the old Clerk session gate): there is no session here,
// so this is a FIRE-ONCE capture check, not a live one. `get_account_email()`
// is read exactly once on mount; `null` shows the inline capture screen,
// anything else renders `children` and this never checks again for the rest
// of the window's life — there is no sign-out to re-lock against.
//
// Loaded only through the `next/dynamic({ ssr: false })` boundary in
// `../signin.tsx` (as `RequireEmailCapture`) — never imported directly — for
// the same static-export reason documented there.

import { useEffect, useState, type ReactNode } from 'react'
import { invoke, isTauri } from '@/lib/bridge'
import { OtpForm } from './OtpForm'

export type CaptureState = 'loading' | 'needs_capture' | 'ready'

/** Resolve the gate's state from a `get_account_email()`-shaped asker,
 *  FAILING OPEN.
 *
 *  Deliberately inverted from the deleted `resolveSignInRequired`'s
 *  fail-CLOSED rule: this is a one-time capture, not a live session gate, so
 *  a rejected `invoke` must land on `'ready'`, never `'needs_capture'`. There
 *  is no session to bypass here — a transient IPC hiccup re-locking an
 *  already-captured user would just mean "ask again once, harmlessly," not a
 *  security hole. The old gate's fail-closed default protected a real,
 *  currently-enforced requirement (a live sign-in); this one only ever
 *  protects a first impression.
 *
 *  `ask` is injectable, mirroring `resolveSignInRequired`'s own reason for
 *  taking its asker as a parameter — the rejection branch is the one that
 *  matters and the one otherwise impossible to reach in a test. */
export async function resolveCaptureState(
  ask: () => Promise<string | null> = () => invoke<string | null>('get_account_email'),
): Promise<CaptureState> {
  try {
    const email = await ask()
    return email ? 'ready' : 'needs_capture'
  } catch {
    return 'ready'
  }
}

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

/** The inline capture screen shown until an email has ever been saved. Reuses
 *  the same OTP form as the wizard and Settings, wrapped in a titled card so
 *  it reads as a deliberate lock, not a stray form. */
function LockedCaptureScreen({ onSignedIn }: { onSignedIn: (email: string) => void }) {
  return (
    <FullScreenCenter>
      <div style={{ width: '100%', maxWidth: 380 }}>
        <div style={{ textAlign: 'center', marginBottom: 22 }}>
          <h1 className="mt-title-lg" style={{ color: 'var(--t-title)' }}>Sign in to Meridian</h1>
          <p className="mt-body-sm mt-2" style={{ color: 'var(--t-muted)' }}>
            Sign in to use Meridian - we&apos;ll email you a one-time code, no password needed.
          </p>
        </div>
        <OtpForm onSignedIn={onSignedIn} />
      </div>
    </FullScreenCenter>
  )
}

function Gate({ children }: { children: ReactNode }) {
  const [state, setState] = useState<CaptureState>('loading')

  useEffect(() => {
    let live = true
    resolveCaptureState().then((s) => { if (live) setState(s) })
    return () => { live = false }
  }, [])

  if (state === 'loading') return <FullScreenCenter>{null}</FullScreenCenter>
  if (state === 'needs_capture') {
    return <LockedCaptureScreen onSignedIn={() => setState('ready')} />
  }
  return <>{children}</>
}

/** Wrap the dashboard app in this to make email capture compulsory. Outside
 *  the Tauri webview (a plain browser preview of the static export) there's
 *  no bridge for `get_account_email` to reach — this shows the same static
 *  notice the old `ClerkGate` used rather than attempting (and failing) an
 *  invoke, since the app only ever runs inside Tauri in practice. */
export function RequireEmailCapture({ children }: { children: ReactNode }) {
  if (!isTauri()) {
    return <p style={{ fontSize: 12.5, color: 'var(--t-muted)' }}>Open Meridian to sign in.</p>
  }
  return <Gate>{children}</Gate>
}
