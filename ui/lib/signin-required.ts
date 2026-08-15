//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// Is sign-in required in this build? — asked by every surface that touches Clerk.
//
// The answer comes from `commands::account::sign_in_required` (Rust), which
// returns false ONLY for a debug build with no Clerk publishable key configured
// — the fresh-clone contributor case, where requiring sign-in would mean
// requiring a key we cannot ship. A packaged build is compiled in release, so
// the bypass branch does not exist in the shipped binary at all.
//
// THREE surfaces need this and they must agree, because they gate the same
// session from different angles:
//   - `RequireSignIn`        hides the whole dashboard
//   - the setup wizard step  blocks Continue (`steps.tsx`'s SIGNIN_STEP)
//   - `AccountAuthControl`   renders Clerk hooks in Settings → Account
// They each rolled their own `invoke(...).then(setState)` first, and the copies
// were NOT identical: two had no `.catch`, so a rejected command left the
// promise unsettled and the surface stuck blank forever. One hook, one
// fail-closed rule, one place to change it.

import { useEffect, useState } from 'react'
import { invoke } from '@/lib/bridge'

/** Resolve the answer, FAILING CLOSED.
 *
 *  A command that cannot be reached must land on the gate, never past it — an
 *  unreachable Rust side is a broken build, not a licence to skip sign-in. This
 *  is the whole reason the function exists rather than a bare `invoke` call.
 *
 *  `ask` is injectable so the fail-closed path is testable without a Tauri
 *  runtime or module mocking — the rejection branch is the one that matters and
 *  the one that is otherwise impossible to reach in a test. */
export async function resolveSignInRequired(
  ask: () => Promise<boolean> = () => invoke<boolean>('sign_in_required'),
): Promise<boolean> {
  try {
    return await ask()
  } catch {
    return true
  }
}

/** `null` while resolving, then the answer.
 *
 *  Callers must treat `null` as "not yet known" and render neither the gated
 *  content nor a Clerk-dependent component — NOT as a falsy "no sign-in
 *  needed", which would flash the app open before the answer arrives. */
export function useSignInRequired(): boolean | null {
  const [required, setRequired] = useState<boolean | null>(null)
  useEffect(() => {
    let live = true
    resolveSignInRequired().then((r) => {
      if (live) setRequired(r)
    })
    return () => {
      live = false
    }
  }, [])
  return required
}
