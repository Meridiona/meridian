//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// Presentational pieces shared by both sign-in surfaces: the loading gate and
// the "who am I signed in as" identity row (avatar + email + caption + a
// trailing action slot).

import type { ReactNode } from 'react'
import { Spinner } from '../atoms'

export function GateLoading() {
  return (
    <div className="flex flex-col items-center justify-center" style={{ minHeight: 120, gap: 12 }}>
      <Spinner size={22} width={2} />
      <p style={{ fontSize: 12.5, color: 'var(--t-faint)' }}>Loading…</p>
    </div>
  )
}

/** First one or two letters for an avatar glyph — `first.last@x` → `FL`,
 *  anything without a clean split (`first@x`) → first two chars of the local
 *  part. Never fabricates a name Clerk didn't give us; the email is the only
 *  identity we have. */
function emailInitials(email: string): string {
  const local = email.split('@')[0] ?? ''
  const parts = local.split(/[._+-]+/).filter(Boolean)
  const raw = parts.length >= 2 ? parts[0][0] + parts[1][0] : local.slice(0, 2)
  return raw.toUpperCase() || '?'
}

function AccountAvatar({ email }: { email: string }) {
  return (
    <div
      aria-hidden="true"
      className="flex items-center justify-center shrink-0 rounded-full"
      style={{
        width: 36, height: 36, fontSize: 13, fontWeight: 600, letterSpacing: 0.2,
        background: 'color-mix(in srgb, var(--color-state-proposal) 16%, transparent)',
        color: 'var(--color-state-proposal)',
      }}
    >
      {emailInitials(email)}
    </div>
  )
}

/** The standard "who am I signed in as" row (Linear/Notion/Raycast-style
 *  account rows): avatar, email, a caption for how the session was
 *  established, and a trailing action slot — a live Sign out button once
 *  Clerk has loaded, or a spinner for an optimistic pre-load render. */
export function AccountIdentityRow({ email, caption, action }: {
  email: string
  caption: string
  action: ReactNode
}) {
  return (
    <div className="flex items-center justify-between" style={{ gap: 16 }}>
      <div className="flex items-center min-w-0" style={{ gap: 12 }}>
        <AccountAvatar email={email} />
        <div className="min-w-0">
          <p className="mt-body-sm font-medium truncate" style={{ color: 'var(--t-title)' }}>{email}</p>
          <p style={{ fontSize: 11, color: 'var(--t-faint)', marginTop: 1 }}>{caption}</p>
        </div>
      </div>
      {action}
    </div>
  )
}
