//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Shared overlay chrome for the wrapper modals (Cleanup / Settings / Plan /
// Tasks): a position:absolute inset-0 backdrop + a centered scrollable panel,
// Escape-to-close, backdrop-click-to-close. Convention mirrors HygieneDialog.
// The wrapped view components render unchanged inside `children`.

'use client'

import { useEffect } from 'react'

export function ModalShell({ title, onClose, children, maxWidth = 720, scrollInside = false, lock }: {
  title: string
  onClose: () => void
  children: React.ReactNode
  /** Turn off every INCIDENTAL way out — the ×, Escape, and the backdrop click —
   *  and replace them with one labelled button saying what leaving means.
   *
   *  For a modal the user was sent to in order to finish something (connecting an
   *  AI provider before a draft can run). Three unlabelled dismissals sitting
   *  around a screen someone was just redirected to is how they end up back at
   *  the button that sent them, with no idea what happened; a stray backdrop
   *  click costs them the whole explanation.
   *
   *  `required` goes further and removes the exit itself, for a step the flow
   *  genuinely cannot continue past — during the walkthrough, an AI engine is
   *  not optional, and "I'll do this later" hands someone a product whose
   *  summaries then silently never run.
   *
   *  IT IS STILL NOT A BLANK WALL, and must never become one. The button stays
   *  on screen, DISABLED, wearing the label as a status — so the corner says
   *  what would unlock it rather than going quiet. The owner is expected to drop
   *  `required` the moment the step is satisfied (see [`SettingsModal`]), and
   *  the walkthrough's own Skip sits above this at z-9000, so the app can never
   *  become unusable even if that release never fires. */
  lock?: { label: string; required?: boolean }
  maxWidth?: number
  // When the child manages its own internal scroll region(s) (e.g. a two-column
  // board where only the columns should scroll, not the whole modal), pass
  // `scrollInside` so the body wrapper hands over a bounded flex box instead of
  // its own padded `overflow-y-auto` — otherwise nesting an `overflow-y-auto`
  // parent around a `flex-1 min-h-0` child breaks the height chain and the
  // WHOLE modal scrolls instead of just the inner region.
  scrollInside?: boolean
}) {
  useEffect(() => {
    if (lock) return
    function onKey(e: KeyboardEvent) { if (e.key === 'Escape') onClose() }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose, lock])

  return (
    <div className="absolute inset-0 z-40 flex items-start justify-center p-6 sm:p-10 rise"
      style={{ background: 'rgba(20,16,40,0.5)', backdropFilter: 'blur(3px)' }}
      onClick={lock ? undefined : onClose}>
      <div className="w-full rounded-2xl overflow-hidden flex flex-col bg-panel"
        style={{
          // `maxHeight` alone (no fixed `height`) lets the box size to its content
          // up to the cap: short content (e.g. the Daily-plan empty state) never
          // shows a scrollbar, while content that exceeds the cap still gets a
          // definite height here, which is what lets a `scrollInside` child's own
          // `flex-1 min-h-0 overflow-y-auto` region clip and scroll correctly.
          maxWidth, maxHeight: '92%', border: '1px solid var(--t-card-border)', boxShadow: 'var(--mt-modal-shadow)',
        }}
        onClick={e => e.stopPropagation()}>
        <div className="flex items-center justify-between px-7 py-5 border-b shrink-0" style={{ borderColor: 'var(--t-hair)' }}>
          <p className="mt-modal-title text-title">{title}</p>
          <button onClick={onClose} aria-label={lock ? lock.label : 'Close'}
            // The first-run walkthrough waits on this to know the user is done
            // with a Settings section it opened (provider pick, tracker
            // connect) — see components/tutorial/script.ts. Kept MOUNTED in
            // every mode, including `required`, for two reasons: a locked modal
            // still ends that beat when the user leaves, and the tour's
            // `waitForElement` gives up after 2s, so a corner that renders
            // nothing until the step completes would make it rush straight past
            // this beat instead of waiting.
            data-tour="modal-close"
            disabled={!!lock?.required}
            className={lock
              ? 'mt-body-sm px-3 py-1.5 rounded-lg bg-wrap'
              : 'inline-flex items-center justify-center rounded-full bg-wrap'}
            style={lock
              ? {
                  color: 'var(--t-muted)', fontWeight: 700,
                  opacity: lock.required ? 0.45 : 1,
                  cursor: lock.required ? 'default' : 'pointer',
                }
              : { width: 30, height: 30, color: 'var(--t-muted)' }}>
            {lock ? lock.label : <span className="text-[17px] leading-none">×</span>}
          </button>
        </div>
        {scrollInside
          ? <div className="flex-1 min-h-0 flex flex-col">{children}</div>
          : <div className="overflow-y-auto nice-scroll p-7">{children}</div>}
      </div>
    </div>
  )
}
