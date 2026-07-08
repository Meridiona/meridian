//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The one standardised "action card" — the single visual language for every
// attention item in the Overview action stack (must-fix, board cleanup, drafts
// to review). Deliberately UNIFORM: cards differ only by icon, copy, and their
// order in the stack — never by colour tier. Urgency is carried by the push
// notification and by the priority order, so the cards themselves stay calm and
// scannable. `onDismiss` is optional; when provided a "Snooze" action appears
// — explicit label instead of a bare × so it's clear the card comes back
// later (per-kind interval) rather than being dismissed for good.

import type { ActionItem } from './useActionItems'

export function ActionCard({ item, onOpen, onDismiss }: {
  item: ActionItem
  onOpen: () => void
  onDismiss?: () => void
}) {
  // A true flex sibling (not an absolutely-positioned overlay) so it's always
  // vertically centered next to the CTA and never overlaps it, regardless of
  // whether the title/subtitle wraps to one or two lines.
  return (
    <div className="w-full rounded-xl bg-card flex items-stretch" style={{ border: '1px solid var(--t-card-border)' }}>
      <button onClick={onOpen}
        className="flex-1 min-w-0 text-left px-4 py-3 flex items-center gap-2.5 transition-transform active:scale-[.99]">
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
          aria-label="Snooze this card"
          className="mt-body-sm shrink-0 px-3 flex items-center justify-center leading-none border-l"
          style={{ color: 'var(--t-faint)', borderColor: 'var(--t-card-border)' }}
        >Snooze</button>
      )}
    </div>
  )
}
