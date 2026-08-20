//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// The quiet "go back one screen" control used across the AI-provider flow.
//
// # Why this exists
// Four screens each hand-rolled the same button: identical chevron path, identical
// 13px box, identical muted inline style, differing only in the label and in
// whether the author happened to put a `{' '}` between the icon and the text. That
// last difference is the tell - the copies had already begun to drift in a way
// nobody would ever notice, which is exactly how a shared control ends up looking
// subtly different on one screen out of four.
//
// It is deliberately NOT a general-purpose button. It carries one visual role -
// secondary, muted, self-start, no fill - and any screen that needs something
// louder should use a real button rather than widening this.
//
// # Who calls this
// `LlmProviderPicker` (four times: the gate's "I don't have one", the opened-vendor detail
// view, the no-subscription preset chooser, and the unknown-vendor fallback - a vendor string
// this build has no display data for), `LlmProviderDetail`, `CloudKeySetup`.

import type { ReactNode } from 'react'

export function BackLink({ onClick, children }: {
  onClick: () => void
  /** The destination, not the gesture - "All providers" reads better than "Back"
   *  when the user may have arrived from more than one place. */
  children: ReactNode
}) {
  return (
    <button
      onClick={onClick}
      className="self-start flex items-center"
      style={{
        gap: 6, fontSize: 12, color: 'var(--t-muted)',
        background: 'none', border: 'none', cursor: 'pointer', padding: 0,
      }}
    >
      <svg
        aria-hidden
        width="13" height="13" viewBox="0 0 16 16" fill="none"
        stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round"
      >
        <path d="M10 4 6 8l4 4" />
      </svg>
      {children}
    </button>
  )
}
