//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// In-app replacements for `window.confirm()` and `window.alert()`, which do
// NOTHING in the packaged tray.
//
// WKWebView routes JS dialogs through the host app's `WKUIDelegate`. Nothing in
// this stack installs one - grepped `wry`, `tauri-runtime-wry`, `tauri` and
// `tauri-utils` for `ConfirmPanel|AlertPanel|WKUIDelegate` and every one is
// empty - so WKWebView silently returns the default: `confirm()` is always
// `false`, `alert()` is a no-op.
//
// That is worse than a visible failure. A falsy `confirm()` is indistinguishable
// from the user clicking Cancel, so the gated action just quietly never runs and
// every log, gate and test still passes. It shipped exactly that way in the
// `db.corrupt` Repair Database button, whose consent gate could never return
// true; the recovery had to be driven by hand from a terminal. The timeline's
// worklog actions had the mirror-image bug - every failure was reported through
// `alert()`, so approve/reject/rematch errors were invisible.
//
// `tauri-plugin-dialog` would also fix it, but it is not a dependency and adding
// one costs a capability grant for a confirmation box the webview can render
// itself. Hence this.
//
// # Who calls this
//
// - `ConfirmDialog` - `NoticeBar`'s db.corrupt repair button.
// - `AlertDialog` - `MeridianTimelineShell`, for worklog action failures.
//
// Any new consent gate or error report in the dashboard should use these -
// `__tests__/no-native-dialogs.test.ts` fails the build if the native globals
// come back.

'use client'

import { useEffect, useRef } from 'react'

/** Ask the user to agree to something before it happens. */
export default function ConfirmDialog({
  title,
  body,
  confirmLabel,
  cancelLabel = 'Cancel',
  busyLabel,
  busy = false,
  error,
  onConfirm,
  onCancel,
}: {
  title: string
  body: string
  confirmLabel: string
  cancelLabel?: string
  /** Shown on the confirm button while `busy`. Defaults to `confirmLabel`. */
  busyLabel?: string
  /** Blocks Escape, the backdrop and both buttons while an action is in flight. */
  busy?: boolean
  /**
   * Failure text, rendered in the dialog itself. There is nowhere else to put
   * it: `window.alert` is inert, and closing the dialog to report the error
   * would leave the user with a banner and no explanation.
   */
  error?: string
  onConfirm: () => void
  onCancel: () => void
}) {
  return (
    <DialogShell title={title} body={body} error={error} busy={busy} onDismiss={onCancel}>
      <button
        onClick={onCancel}
        disabled={busy}
        className="text-[12px] px-3.5 py-2 rounded-lg border transition-colors"
        style={{ borderColor: 'var(--rule)', color: 'var(--ink-2)', background: 'var(--paper)' }}
      >
        {cancelLabel}
      </button>
      <PrimaryButton autoFocus disabled={busy} onClick={onConfirm}>
        {busy ? (busyLabel ?? confirmLabel) : confirmLabel}
      </PrimaryButton>
    </DialogShell>
  )
}

/** Tell the user something went wrong. One button, nothing to decide. */
export function AlertDialog({
  title,
  body,
  dismissLabel = 'OK',
  onDismiss,
}: {
  title: string
  body: string
  dismissLabel?: string
  onDismiss: () => void
}) {
  return (
    <DialogShell title={title} body={body} onDismiss={onDismiss}>
      <PrimaryButton autoFocus onClick={onDismiss}>{dismissLabel}</PrimaryButton>
    </DialogShell>
  )
}

// Modal chrome shared by both: backdrop, card, Escape and click-outside. Kept
// private so every dialog in the app behaves identically.
function DialogShell({
  title,
  body,
  error,
  busy = false,
  onDismiss,
  children,
}: {
  title: string
  body: string
  error?: string
  busy?: boolean
  onDismiss: () => void
  children: React.ReactNode
}) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && !busy) onDismiss()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [busy, onDismiss])

  return (
    <div
      className="fixed inset-0 z-[100] flex items-center justify-center p-4 rise"
      style={{ background: 'rgba(20,16,10,0.45)', backdropFilter: 'blur(3px)' }}
      onClick={() => { if (!busy) onDismiss() }}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label={title}
        className="w-full max-w-md rounded-[20px] overflow-hidden"
        style={{
          background: 'var(--paper)',
          border: '1px solid var(--rule)',
          boxShadow: '0 24px 60px -12px rgba(20,16,10,0.35)',
        }}
        onClick={(e) => e.stopPropagation()}
      >
        <div style={{ height: 3, background: 'linear-gradient(90deg, var(--accent), var(--warn))' }} />

        <div className="px-6 pt-5 pb-4">
          <h2 className="text-[15px] font-semibold leading-snug" style={{ color: 'var(--ink)' }}>
            {title}
          </h2>
          <p className="text-[13px] mt-2 whitespace-pre-line" style={{ color: 'var(--ink-2)' }}>
            {body}
          </p>
          {error && (
            <p
              className="text-[12px] mt-3 px-3 py-2 rounded-lg"
              style={{ background: 'var(--warn)' + '1A', color: 'var(--warn)' }}
            >
              {error}
            </p>
          )}
        </div>

        <div
          className="px-6 py-4 flex items-center justify-end gap-2"
          style={{ borderTop: '1px solid var(--rule)', background: 'var(--surface)' }}
        >
          {children}
        </div>
      </div>
    </div>
  )
}

// Focused on mount so the dialog is operable from the keyboard the moment it
// opens, the way a native one would be.
function PrimaryButton({
  autoFocus,
  disabled = false,
  onClick,
  children,
}: {
  autoFocus?: boolean
  disabled?: boolean
  onClick: () => void
  children: React.ReactNode
}) {
  const ref = useRef<HTMLButtonElement>(null)
  useEffect(() => {
    if (autoFocus) ref.current?.focus()
  }, [autoFocus])

  return (
    <button
      ref={ref}
      onClick={onClick}
      disabled={disabled}
      className="text-[12px] font-medium px-3.5 py-2 rounded-lg transition-colors"
      style={{ background: disabled ? 'var(--rule)' : 'var(--accent)', color: disabled ? 'var(--ink-4)' : '#fff' }}
    >
      {children}
    </button>
  )
}
