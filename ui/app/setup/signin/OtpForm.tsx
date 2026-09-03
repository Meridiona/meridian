//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// Email + code form — sends/verifies a one-time code via the OTP Worker
// (tray/src-tauri/src/commands/otp.rs: request_account_otp / confirm_account_otp).
// Replaces EmailCodeForm.tsx (the old @clerk/react-backed form): no client auth
// library, no session object, no async plugin-init step — every call here is a
// plain Tauri `invoke`, so unlike the form it replaces this needs no surrounding
// gate/boundary to bootstrap against (see RequireEmailCapture.tsx, which is
// simpler than the ClerkGate it replaces for the same reason).

import { useEffect, useRef, useState } from 'react'
import type { CSSProperties } from 'react'
import { invoke } from '@/lib/bridge'
import { classifySendError, classifyVerifyError } from '@/lib/otp-errors'
import { Btn, PermIcon, Spinner } from '../atoms'

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
      onFocus={(e) => { e.target.style.borderColor = 'var(--t-accent)' }}
      onBlur={(e) => { e.target.style.borderColor = 'var(--t-input-border)' }}
    />
  )
}

/** Standard OTP-industry resend cooldown (Google/GitHub/Stripe et al. all
 *  gate resend at ~30s) — stops a mis-tap or impatience from spamming the
 *  Worker's send endpoint (and racking up its per-email/per-IP KV rate-limit
 *  caps) for no benefit, since a code already in flight is usually still on
 *  its way. */
const RESEND_COOLDOWN_S = 30

export function OtpForm({ onSignedIn, onDevBypass }: {
  onSignedIn: (email: string) => void
  /** Fired the first time a send/verify attempt reports the Worker isn't
   *  configured (`OTP_API_URL` unset/blank) — the fresh-clone dev case. Only
   *  meaningful to the wizard step (`steps.tsx`'s `EMAIL_STEP.canNext`), which
   *  uses it to stop blocking Next even though no email was ever captured.
   *  Omitted by `AccountAuthControl.tsx`'s "Change email" use, since a signed-in
   *  user reaching "not configured" there is a genuine misconfiguration with
   *  nothing to unblock. */
  onDevBypass?: () => void
}) {
  const [phase, setPhase] = useState<'email' | 'code'>('email')
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
  // state updates are async, ref writes aren't.
  const submitting = useRef(false)

  const submitEmail = async () => {
    if (submitting.current) return
    submitting.current = true
    setBusy(true); setErr('')
    try {
      await invoke('request_account_otp', { email })
      setPhase('code')
      setResendReadyAt(Date.now() + RESEND_COOLDOWN_S * 1000)
    } catch (e) {
      const { message, isDevBypass } = classifySendError(e)
      setErr(message)
      if (isDevBypass) onDevBypass?.()
    } finally {
      submitting.current = false
      setBusy(false)
    }
  }

  const submitCode = async () => {
    if (submitting.current) return
    submitting.current = true
    setBusy(true); setErr('')
    try {
      const verified = await invoke<boolean>('confirm_account_otp', { email, code })
      if (verified) {
        // Best-effort, like every other `save_account_email` call site
        // (e.g. `page.tsx`'s `onSignedIn`) — a failed write here still lets
        // the user through; the wizard/dashboard's own `onSignedIn` may retry
        // the save independently.
        await invoke('save_account_email', { email }).catch(() => {})
        onSignedIn(email)
        return
      }
      // A wrong-but-well-formed code is `Ok(false)`, not a rejection — the
      // retryable, expected outcome copy lives here rather than in
      // `classifyVerifyError`.
      setErr("That code didn't work - check it and try again.")
    } catch (e) {
      const { message, isDevBypass } = classifyVerifyError(e)
      setErr(message)
      if (isDevBypass) onDevBypass?.()
    } finally {
      submitting.current = false
      setBusy(false)
    }
  }

  /** Re-sends a fresh code to the same address — separate from "Use a
   *  different email" (which resets back to the email step) so a simply
   *  expired or already-used code doesn't force re-typing the email. Gated by
   *  `resendCooldownS` (the button itself stays disabled until it hits zero),
   *  but re-checked here too since disabled state lags a render behind the
   *  ticking clock. */
  const resendCode = async () => {
    if (submitting.current || resendCooldownS > 0) return
    submitting.current = true
    setBusy(true); setErr('')
    try {
      await invoke('request_account_otp', { email })
      setCode('')
      setResendReadyAt(Date.now() + RESEND_COOLDOWN_S * 1000)
    } catch (e) {
      const { message, isDevBypass } = classifySendError(e)
      setErr(message)
      if (isDevBypass) onDevBypass?.()
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
            background: 'color-mix(in srgb, var(--t-accent) 12%, transparent)',
            color: 'var(--t-accent)',
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
