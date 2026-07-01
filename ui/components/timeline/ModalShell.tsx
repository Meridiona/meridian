//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Shared overlay chrome for the wrapper modals (Cleanup / Settings / Plan /
// Tasks): a position:absolute inset-0 backdrop + a centered scrollable panel,
// Escape-to-close, backdrop-click-to-close. Convention mirrors HygieneDialog.
// The wrapped view components render unchanged inside `children`.

'use client'

import { useEffect } from 'react'

export function ModalShell({ title, onClose, children, maxWidth = 720 }: {
  title: string
  onClose: () => void
  children: React.ReactNode
  maxWidth?: number
}) {
  useEffect(() => {
    function onKey(e: KeyboardEvent) { if (e.key === 'Escape') onClose() }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose])

  return (
    <div className="absolute inset-0 z-40 flex items-start justify-center p-4 sm:p-8 rise"
      style={{ background: 'rgba(20,16,40,0.5)', backdropFilter: 'blur(3px)' }} onClick={onClose}>
      <div className="w-full rounded-2xl overflow-hidden flex flex-col bg-panel"
        style={{ maxWidth, maxHeight: '92%', border: '1px solid var(--t-card-border)', boxShadow: '0 24px 60px -18px rgba(20,16,40,0.5)' }}
        onClick={e => e.stopPropagation()}>
        <div className="flex items-center justify-between px-5 py-3.5 border-b shrink-0" style={{ borderColor: 'var(--t-hair)' }}>
          <p className="mt-modal-title text-title">{title}</p>
          <button onClick={onClose} aria-label="Close"
            className="inline-flex items-center justify-center rounded-full bg-wrap"
            style={{ width: 30, height: 30, color: 'var(--t-muted)' }}>
            <span className="text-[17px] leading-none">×</span>
          </button>
        </div>
        <div className="overflow-y-auto nice-scroll p-5">{children}</div>
      </div>
    </div>
  )
}
