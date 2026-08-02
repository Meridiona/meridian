//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// Email + code form — the actual sign-in/up custom flow. Uses @clerk/react's
// "Future" resource API (signIn.emailCode.*, signUp.verifications.*,
// .finalize() to activate the session) — the older prepareFirstFactor/
// attemptFirstFactor + setActive() API this version of @clerk/react
// replaced. Errors come back as `{ error }`, not thrown.

import { useEffect, useRef, useState } from 'react'
import type { CSSProperties } from 'react'
import { useSignIn, useSignUp } from '@clerk/react'
import { Btn, PermIcon, Spinner } from '../atoms'
import { clerkErrorCode, clerkErrorMessage } from './errors'

/** A bespoke input for this form rather than the shared `<TextInput>` (used
 *  elsewhere for compact settings rows, 12px/5px padding) - an auth form is
 *  the one place in the wizard that warrants its own larger, more deliberate
 *  input treatment. `focusRing` is applied via onFocus/onBlur rather than a
 *  CSS class since this file has no stylesheet of its own. */
function AuthInput(props: {
  value: string
  onChange: (v: string) => void
  onEnter: () => void
  type: 'email' | 'text'
  placeholder: string
  style?: CSSProperties
}) {
  return (
    <input
      type={props.type}
      inputMode={props.type === 'text' ? 'numeric' : undefined}
      value={props.value}
      onChange={(e) => props.onChange(e.target.value)}
      onKeyDown={(e) => e.key === 'Enter' && props.onEnter()}
      placeholder={props.placeholder}
      autoFocus
      style={{
        width: '100%', fontSize: 14, padding: '11px 14px',
        background: 'var(--t-input)', color: 'var(--t-title)',
        border: '1px solid var(--t-input-border)', borderRadius: 10,
        outline: 'none', fontFamily: 'inherit', textAlign: 'center',
        transition: 'border-color .14s',
        ...props.style,
      }}
      onFocus={(e) => { e.target.style.borderColor = 'var(--color-state-proposal)' }}
      onBlur={(e) => { e.target.style.borderColor = 'var(--t-input-border)' }}
    />
  )
}

/** Standard OTP-industry resend cooldown (Google/GitHub/Stripe et al. all
 *  gate resend at ~30s) — stops a mis-tap or impatience from spamming Clerk's
 *  email-send endpoint (and racking up its per-instance email quota) for no
 *  benefit, since a code already in flight is usually still on its way. */
const RESEND_COOLDOWN_S = 30

export function EmailCodeForm() {
  const { signIn } = useSignIn()
  const { signUp } = useSignUp()
  const [phase, setPhase] = useState<'email' | 'code'>('email')
  const [mode, setMode] = useState<'signIn' | 'signUp' | null>(null)
  const [email, setEmail] = useState('')
  const [code, setCode] = useState('')
  const [busy, setBusy] = useState(false)
  const [err, setErr] = useState('')
  // Epoch ms when "Resend code" re-enables; null = no cooldown active.
  const [resendReadyAt, setResendReadyAt] = useState<number | null>(null)
  // Bumped every second while a cooldown is active purely to force the
  // countdown label to re-render — `resendReadyAt` itself doesn't change.
  const [, forceTick] = useState(0)

  useEffect(() => {
    if (!resendReadyAt) return
    const id = setInterval(() => {
      // Once the cooldown elapses, clear `resendReadyAt` so this effect
      // re-runs and tears the interval down — otherwise it would keep forcing
      // a re-render every second forever after the countdown hits zero.
      if (Date.now() >= resendReadyAt) setResendReadyAt(null)
      else forceTick((n) => n + 1)
    }, 1000)
    return () => clearInterval(id)
  }, [resendReadyAt])

  const resendCooldownS = resendReadyAt ? Math.max(0, Math.ceil((resendReadyAt - Date.now()) / 1000)) : 0
  // A ref, not just the `busy` state, guards against double-submit: a fast
  // double-click/tap can fire a second `onClick` before React has re-rendered
  // the button's `disabled` prop from the first click's `setBusy(true)` —
  // state updates are async, ref writes aren't. Without this a rapid double
  // click sends the same code to Clerk twice; the first call actually
  // succeeds, the second correctly bounces off as "already verified", and
  // that error used to surface even though sign-in had already worked.
  const submitting = useRef(false)

  const submitEmail = async () => {
    if (submitting.current) return
    submitting.current = true
    setBusy(true); setErr('')
    try {
      const created = await signIn.create({ identifier: email })
      // No matching account — fall back to sign-up so one form handles both
      // first-time and returning users.
      if (clerkErrorCode(created.error) === 'form_identifier_not_found' || signIn.isTransferable) {
        const su = await signUp.create({ emailAddress: email })
        if (su.error) { setErr(clerkErrorMessage(su.error, 'Could not send a code - try again.')); return }
        const sent = await signUp.verifications.sendEmailCode()
        if (sent.error) { setErr(clerkErrorMessage(sent.error, 'Could not send a code - try again.')); return }
        setMode('signUp'); setPhase('code'); setResendReadyAt(Date.now() + RESEND_COOLDOWN_S * 1000)
        return
      }
      if (created.error) { setErr(clerkErrorMessage(created.error, 'Could not send a code - try again.')); return }
      const sent = await signIn.emailCode.sendCode()
      if (sent.error) { setErr(clerkErrorMessage(sent.error, 'Could not send a code - try again.')); return }
      setMode('signIn'); setPhase('code'); setResendReadyAt(Date.now() + RESEND_COOLDOWN_S * 1000)
    } catch (e) {
      // Clerk's Future API returns { error } rather than throwing, but a
      // transport-level failure still rejects. Catch it here so it surfaces to
      // the user instead of bubbling as an unhandled rejection (the
      // ClerkErrorBoundary only covers render/use() failures, not these
      // async handlers).
      // eslint-disable-next-line no-console -- only diagnostic for this failure
      console.error('sign-in: submitEmail threw', e)
      setErr('Something went wrong - check your connection and try again.')
    } finally {
      submitting.current = false
      setBusy(false)
    }
  }

  const submitCode = async () => {
    if (submitting.current || !mode) return
    submitting.current = true
    setBusy(true); setErr('')
    try {
      const resource = mode === 'signIn' ? signIn : signUp
      const verified = mode === 'signIn'
        ? await signIn.emailCode.verifyCode({ code })
        : await signUp.verifications.verifyEmailCode({ code })
      if (verified.error) {
        // A prior submit of this exact code already completed sign-in (see
        // `submitting` above, or a second device/window racing the same
        // code) — there's nothing left to do; the session shows up via
        // useUser() a moment later. Anything else is a real failure — logged
        // to the console since Clerk's specific reason (wrong digits,
        // expired code, too many attempts) is more useful for debugging than
        // the generic fallback shown in the UI.
        if (clerkErrorCode(verified.error) !== 'verification_already_verified') {
          // eslint-disable-next-line no-console -- only diagnostic surfaced anywhere for this failure
          console.error('sign-in: code verification failed', verified.error)
          setErr(clerkErrorMessage(verified.error, "That code didn't work - check it and try again."))
        }
        return
      }
      if (resource.status !== 'complete') {
        // `missing_requirements` means the code was right but Clerk wants
        // more fields (password, name, …) than this passwordless/email-only
        // form ever collects — a Clerk dashboard config gap (an auth
        // requirement enabled that we don't ask for), not a code bug. Log
        // exactly which fields so it's fixable without guessing.
        const missing = 'missingFields' in resource ? resource.missingFields : undefined
        // eslint-disable-next-line no-console -- see above
        console.error('sign-in: verification resolved without completing', { status: resource.status, missing })
        setErr(
          resource.status === 'missing_requirements'
            ? "Your code verified, but sign-in isn't fully configured for email-only access yet - see the console for details."
            : "That code didn't work - check it and try again.",
        )
        return
      }
      const finalized = await resource.finalize()
      if (finalized.error) setErr(clerkErrorMessage(finalized.error, 'Could not complete sign-in - try again.'))
    } catch (e) {
      // A transport-level rejection (see submitEmail's note) — surface it
      // rather than letting it escape as an unhandled rejection.
      // eslint-disable-next-line no-console -- see above
      console.error('sign-in: submitCode threw', e)
      setErr('Something went wrong - check your connection and try again.')
    } finally {
      submitting.current = false
      setBusy(false)
    }
  }

  /** Re-sends a fresh code to the same address/resource — separate from
   *  "Use a different email" (which resets back to the email step) so a
   *  simply-expired or already-used code doesn't force re-typing the email.
   *  Gated by `resendCooldownS` (the button itself stays disabled until it
   *  hits zero), but re-checked here too since disabled state lags a render
   *  behind the ticking clock. */
  const resendCode = async () => {
    if (submitting.current || !mode || resendCooldownS > 0) return
    submitting.current = true
    setBusy(true); setErr('')
    try {
      const resent = mode === 'signIn'
        ? await signIn.emailCode.sendCode()
        : await signUp.verifications.sendEmailCode()
      if (resent.error) {
        // eslint-disable-next-line no-console -- see submitCode's note above
        console.error('sign-in: resend failed', resent.error)
        setErr(clerkErrorMessage(resent.error, 'Could not resend the code - try again.'))
        return
      }
      setCode('')
      setResendReadyAt(Date.now() + RESEND_COOLDOWN_S * 1000)
    } catch (e) {
      // A transport-level rejection (see submitEmail's note) — surface it
      // rather than letting it escape as an unhandled rejection.
      // eslint-disable-next-line no-console -- see above
      console.error('sign-in: resendCode threw', e)
      setErr('Could not resend the code - check your connection and try again.')
    } finally {
      submitting.current = false
      setBusy(false)
    }
  }

  const canSubmit = phase === 'email' ? email.includes('@') : code.trim().length > 0

  return (
    <div className="flex flex-col items-center" style={{ width: '100%', maxWidth: 340, margin: '0 auto' }}>
      <div className="w-full" style={{
        borderRadius: 16, padding: '26px 26px 22px', border: '0.5px solid var(--t-card-border)',
        background: 'var(--t-card)', boxShadow: '0 1px 3px rgba(0,0,0,.05)',
      }}>
        <div className="flex flex-col items-center mer-pop" style={{ gap: 5, marginBottom: 20, textAlign: 'center' }}>
          <span className="flex items-center justify-center shrink-0" style={{
            width: 42, height: 42, borderRadius: 13, marginBottom: 4,
            background: 'color-mix(in srgb, var(--color-state-proposal) 12%, transparent)',
            color: 'var(--color-state-proposal)',
          }}>
            <PermIcon icon={phase === 'email' ? 'mail' : 'shield'} size={19} />
          </span>
          <p style={{ fontSize: 14.5, fontWeight: 600, color: 'var(--t-title)' }}>
            {phase === 'email' ? "What's your email?" : 'Check your inbox'}
          </p>
          <p style={{ fontSize: 12, lineHeight: 1.45, color: 'var(--t-muted)' }}>
            {phase === 'email'
              ? "No password needed - we'll email you a one-time code."
              : <>Enter the code sent to <span style={{ fontWeight: 600, color: 'var(--t-title)' }}>{email}</span></>}
          </p>
        </div>

        <div className="flex flex-col" style={{ gap: 12 }}>
          {phase === 'email' ? (
            <AuthInput type="email" value={email} onChange={setEmail} onEnter={() => canSubmit && !busy && submitEmail()} placeholder="you@example.com" />
          ) : (
            <AuthInput type="text" value={code} onChange={setCode} onEnter={() => canSubmit && !busy && submitCode()} placeholder="123456"
              style={{ fontSize: 20, fontWeight: 600, letterSpacing: '.4em', paddingLeft: 20 }} />
          )}

          <Btn
            onClick={() => (phase === 'email' ? submitEmail() : submitCode())}
            disabled={busy || !canSubmit}
            style={{ width: '100%', padding: '11px', fontSize: 13.5 }}
          >
            {busy ? <Spinner size={14} width={1.8} color="#fff" /> : phase === 'email' ? 'Send code' : 'Verify & continue'}
          </Btn>

          {phase === 'code' && (
            <div className="flex items-center justify-center" style={{ gap: 10, fontSize: 11.5 }}>
              <Btn variant="ghost" size="sm" disabled={busy || resendCooldownS > 0} onClick={resendCode}>
                {resendCooldownS > 0 ? `Resend in ${resendCooldownS}s` : 'Resend code'}
              </Btn>
              <span style={{ color: 'var(--t-faint)' }}>·</span>
              <Btn variant="ghost" size="sm" disabled={busy} onClick={() => { setPhase('email'); setCode(''); setErr('') }}>
                Use a different email
              </Btn>
            </div>
          )}
        </div>

        {err && <p style={{ fontSize: 11, color: 'var(--color-state-pending)', textAlign: 'center', marginTop: 10 }}>{err}</p>}
      </div>

      <p className="flex items-center" style={{ gap: 6, marginTop: 14, fontSize: 11, lineHeight: 1.4, color: 'var(--t-faint)', textAlign: 'center' }}>
        <span className="shrink-0" style={{ color: 'var(--color-state-approved)' }}><PermIcon icon="shield" size={12} /></span>
        We never see your screen or activity - this only tells us who&apos;s signed in.
      </p>
    </div>
  )
}
