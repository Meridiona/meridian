//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// Plain email capture — no verification code. Saves the typed address locally
// via `save_account_email` (tray/src-tauri/src/commands/account.rs) and
// best-effort notifies the team via the OTP Worker's `/otp/capture`
// (tray/src-tauri/src/commands/otp.rs: capture_account_email) — fire-and-forget,
// since a slow/unreachable Worker or SES/Resend must never block sign-in.
// Replaces the old two-step send-code/verify-code flow (itself a replacement
// for EmailCodeForm.tsx, the @clerk/react-backed form): no client auth
// library, no session object, no async plugin-init step, and now no external
// round trip on the capture path either — every call here is a plain Tauri
// `invoke`, so this needs no surrounding gate/boundary to bootstrap against
// (see RequireEmailCapture.tsx, which is simpler than the ClerkGate it
// replaces for the same reason).

import { useRef, useState } from 'react'
import type { CSSProperties } from 'react'
import { invoke } from '@/lib/bridge'
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
  placeholder: string
  style?: CSSProperties
}) {
  return (
    <input
      type="email"
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

const GENERIC_SAVE_ERROR = "That doesn't look like a valid email address."

export function OtpForm({ onSignedIn }: {
  onSignedIn: (email: string) => void
}) {
  const [email, setEmail] = useState('')
  const [busy, setBusy] = useState(false)
  const [err, setErr] = useState('')
  // A ref, not just the `busy` state, guards against double-submit: a fast
  // double-click/tap can fire a second `onClick` before React has re-rendered
  // the button's `disabled` prop from the first click's `setBusy(true)` —
  // state updates are async, ref writes aren't.
  const submitting = useRef(false)

  const canSubmit = email.includes('@')

  const submitEmail = async () => {
    if (submitting.current) return
    submitting.current = true
    setBusy(true); setErr('')
    const trimmed = email.trim()
    try {
      // Fire-and-forget: notifies the team via the Worker's `/otp/capture`
      // (Resend, not SES) that someone signed up. Deliberately not awaited
      // and its rejection is swallowed — capture must succeed locally even
      // when the Worker is unreachable or misconfigured; this is a
      // nice-to-have side effect, never a gate.
      invoke('capture_account_email', { email: trimmed }).catch(() => {})
      await invoke('save_account_email', { email: trimmed })
      onSignedIn(trimmed)
    } catch {
      setErr(GENERIC_SAVE_ERROR)
    } finally {
      submitting.current = false
      setBusy(false)
    }
  }

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
            <PermIcon icon="mail" size={19} />
          </span>
          <p style={{ fontSize: 14.5, fontWeight: 600, color: 'var(--t-title)' }}>
            What&apos;s your email?
          </p>
          <p style={{ fontSize: 12, lineHeight: 1.45, color: 'var(--t-muted)' }}>
            No password or code needed - just tells us who&apos;s signed in.
          </p>
        </div>

        <div className="flex flex-col" style={{ gap: 12 }}>
          <AuthInput value={email} onChange={setEmail} onEnter={() => canSubmit && !busy && submitEmail()} placeholder="you@example.com" />

          <Btn
            onClick={submitEmail}
            disabled={busy || !canSubmit}
            style={{ width: '100%', padding: '11px', fontSize: 13.5 }}
          >
            {busy ? <Spinner size={14} width={1.8} color="#fff" /> : 'Continue'}
          </Btn>
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
