//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Shared overlay chrome for the wrapper modals (Cleanup / Settings / Plan /
// Tasks): a position:absolute inset-0 backdrop + a centered scrollable panel,
// Escape-to-close, backdrop-click-to-close. Convention mirrors HygieneDialog.
// The wrapped view components render unchanged inside `children`.

'use client'

import { useEffect } from 'react'

export function ModalShell({ title, onClose, children, maxWidth = 720, scrollInside = false }: {
  title: string
  onClose: () => void
  children: React.ReactNode
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
    function onKey(e: KeyboardEvent) { if (e.key === 'Escape') onClose() }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose])

  return (
    <div className="absolute inset-0 z-40 flex items-start justify-center p-6 sm:p-10 rise"
      style={{ background: 'rgba(20,16,40,0.5)', backdropFilter: 'blur(3px)' }} onClick={onClose}>
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
          <button onClick={onClose} aria-label="Close"
            className="inline-flex items-center justify-center rounded-full bg-wrap"
            style={{ width: 30, height: 30, color: 'var(--t-muted)' }}>
            <span className="text-[17px] leading-none">×</span>
          </button>
        </div>
        {scrollInside
          ? <div className="flex-1 min-h-0 flex flex-col">{children}</div>
          : <div className="overflow-y-auto nice-scroll p-7">{children}</div>}
      </div>
    </div>
  )
}
