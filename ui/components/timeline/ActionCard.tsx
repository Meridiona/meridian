//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The one standardised "action card" — the single visual language for every
// attention item in the Overview action stack (must-fix, board cleanup, drafts
// to review). Deliberately UNIFORM: cards differ only by icon, copy, and their
// order in the stack — never by colour tier. Urgency is carried by the push
// notification and by the priority order, so the cards themselves stay calm and
// scannable. `onDismiss` is optional; when provided a × appears to snooze the
// card (reappears after a per-kind interval — wired in a later slice).

import type { ActionItem } from './useActionItems'

export function ActionCard({ item, onOpen, onDismiss }: {
  item: ActionItem
  onOpen: () => void
  onDismiss?: () => void
}) {
  return (
    <div className="relative w-full rounded-xl bg-card" style={{ border: '1px solid var(--t-card-border)' }}>
      <button onClick={onOpen}
        className="w-full text-left px-4 py-3 flex items-center gap-2.5 transition-transform active:scale-[.99]">
        <span className="inline-flex items-center justify-center rounded-full shrink-0 text-[13px]"
          style={{ width: 26, height: 26, background: 'color-mix(in srgb, var(--t-title) 8%, transparent)' }}>{item.icon}</span>
        <span className="flex-1 min-w-0">
          <p className="mt-card-title" style={{ color: 'var(--t-title)' }}>{item.title}</p>
          <p className="mt-body-sm mt-0.5" style={{ color: 'var(--t-muted)' }}>{item.subtitle}</p>
        </span>
        <span className="mt-body-sm shrink-0" style={{ color: 'var(--t-faint)' }}>{item.cta} →</span>
      </button>
      {onDismiss && (
        <button
          onClick={onDismiss}
          aria-label="Dismiss for now"
          className="absolute top-1 right-1 w-6 h-6 inline-flex items-center justify-center rounded-md leading-none"
          style={{ color: 'var(--t-faint)' }}
        >×</button>
      )}
    </div>
  )
}
