//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// The setup wizard's Sign-in step body — see `../signin.tsx` for why this
// module (and everything it imports) is only ever loaded through a
// `next/dynamic(..., { ssr: false })` boundary, never imported directly.

import { useSignedInEmail } from './useSignedInEmail'
import { ClerkGate } from './ClerkGate'
import { GateLoading } from './identity'
import { EmailCodeForm } from './EmailCodeForm'

/** No gating: reports a signed-in session up via `onSignedIn` and renders
 *  the form until then. The wizard step (`steps.tsx`'s `SignInBody`) decides
 *  what to show once `onSignedIn` fires — this component never hides the
 *  rest of the wizard itself. */
function SignedInOrForm({ onSignedIn }: { onSignedIn: (email: string) => void }) {
  const { isLoaded, isSignedIn } = useSignedInEmail(onSignedIn)
  if (!isLoaded || isSignedIn) return <GateLoading />
  return <EmailCodeForm />
}

/** The setup wizard's Sign-in step body. Outside the Tauri webview (e.g. a
 *  plain browser preview of the static export) there's no `tauri-plugin-clerk`
 *  bridge to init against, so it degrades to a notice rather than hanging. */
export function SignInWidget({ onSignedIn }: { onSignedIn: (email: string) => void }) {
  return (
    <ClerkGate notInTauriMessage="Open Meridian to sign in." fallback={<GateLoading />}>
      {() => <SignedInOrForm onSignedIn={onSignedIn} />}
    </ClerkGate>
  )
}
