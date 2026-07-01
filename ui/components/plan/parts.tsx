//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// Small presentational helpers shared by the daily-plan columns. Due date is
// given visual weight here (urgency → colour), matching its weight in scoring.
// Themed with the mt-* timeline tokens (see globals.css) — --color-state-pending
// (amber) reads as urgency, --color-state-proposal (purple) as the plan's accent.

/** A due-date pill coloured by urgency. Renders nothing when there's no date. */
export function DuePill({ days }: { days: number | null }) {
  if (days === null) return null

  let label: string
  let color: string
  if (days < 0) { label = `Overdue ${-days}d`; color = 'var(--color-state-pending)' }
  else if (days === 0) { label = 'Due today'; color = 'var(--color-state-pending)' }
  else if (days === 1) { label = 'Due tomorrow'; color = 'var(--color-state-proposal)' }
  else if (days <= 14) { label = `Due ${days}d`; color = 'var(--color-state-proposal)' }
  else if (days <= 30) { label = `Due ${days}d`; color = 'var(--t-muted)' }
  else { label = `Due ${Math.round(days / 7)}w`; color = 'var(--t-faint)' }

  return (
    <span className="mt-chip inline-flex items-center gap-1 px-1.5 py-0.5 rounded whitespace-nowrap"
      style={{ color, background: `color-mix(in srgb, ${color} 12%, transparent)` }}>
      <svg width="9" height="9" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6">
        <rect x="2.5" y="3.5" width="11" height="10" rx="1.5" />
        <path d="M2.5 6.5h11M5.5 1.5v2M10.5 1.5v2" />
      </svg>
      {label}
    </span>
  )
}

/** The lead "why this is here" chip (carried over / in progress / worked recently). */
export function OriginChip({ reason, origin }: { reason: string; origin: string }) {
  const strong = origin === 'carryover' || origin === 'in_progress'
  return (
    <span className="mt-chip px-1.5 py-0.5 rounded whitespace-nowrap"
      style={{
        color: strong ? 'var(--color-state-proposal)' : 'var(--t-faint)',
        background: strong ? 'color-mix(in srgb, var(--color-state-proposal) 12%, transparent)' : 'var(--t-box)',
      }}>
      {reason}
    </span>
  )
}

/** Status / column chip. Accent-tinted when the column reads "in progress". */
export function StatusChip({ status }: { status?: string }) {
  const s = (status || '').trim()
  if (!s) return null
  const active = /progress|doing|review|qa|testing|dev|implement|active|building/i.test(s)
  return (
    <span className="mt-chip px-1.5 py-0.5 rounded whitespace-nowrap"
      style={{
        color: active ? 'var(--color-state-proposal)' : 'var(--t-faint)',
        background: active ? 'color-mix(in srgb, var(--color-state-proposal) 12%, transparent)' : 'var(--t-box)',
      }}>
      {s}
    </span>
  )
}

/** Epic / parent chip. */
export function EpicChip({ epic }: { epic?: string | null }) {
  if (!epic) return null
  const label = epic.length > 22 ? epic.slice(0, 21) + '…' : epic
  return (
    <span className="mt-chip inline-flex items-center gap-1 px-1.5 py-0.5 rounded whitespace-nowrap"
      style={{ color: 'var(--t-faint)', background: 'var(--t-box)' }} title={epic}>
      <svg width="8" height="8" viewBox="0 0 16 16" fill="currentColor" aria-hidden><rect x="2" y="2" width="5" height="5" rx="1" /><rect x="9" y="2" width="5" height="5" rx="1" /><rect x="2" y="9" width="5" height="5" rx="1" /><rect x="9" y="9" width="5" height="5" rx="1" /></svg>
      {label}
    </span>
  )
}

/** Priority dot + label, coloured by urgency. Provider-agnostic string match. */
export function PriorityTag({ priority }: { priority?: string | null }) {
  if (!priority) return null
  const p = priority.toLowerCase()
  const color = /highest|critical|blocker|p1|urgent/.test(p) ? 'var(--color-state-pending)'
    : /high|p2/.test(p) ? 'var(--color-state-proposal)'
      : /low|minor|p4|p5|trivial/.test(p) ? 'var(--t-faint-2)'
        : 'var(--t-faint)'
  return (
    <span className="mt-chip inline-flex items-center gap-1 whitespace-nowrap" style={{ color: 'var(--t-faint)' }} title={`Priority: ${priority}`}>
      <span className="w-1.5 h-1.5 rounded-full" style={{ background: color }} />
      {priority}
    </span>
  )
}

/** Tiny generic chip (e.g. story points, issue type). */
export function MetaChip({ children }: { children: React.ReactNode }) {
  return (
    <span className="mt-chip px-1.5 py-0.5 rounded whitespace-nowrap" style={{ color: 'var(--t-faint)', background: 'var(--t-box)' }}>
      {children}
    </span>
  )
}

/** External "open in tracker" link. */
export function OpenLink({ url }: { url?: string }) {
  if (!url) return null
  return (
    <a href={url} target="_blank" rel="noopener noreferrer" onPointerDown={e => e.stopPropagation()}
      className="mt-chip whitespace-nowrap hover:underline" style={{ color: 'var(--t-faint)' }}>
      Open ↗
    </a>
  )
}

/** Drag-handle glyph. */
export function GripHandle() {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor" aria-hidden>
      <circle cx="6" cy="4" r="1.1" /><circle cx="10" cy="4" r="1.1" />
      <circle cx="6" cy="8" r="1.1" /><circle cx="10" cy="8" r="1.1" />
      <circle cx="6" cy="12" r="1.1" /><circle cx="10" cy="12" r="1.1" />
    </svg>
  )
}
