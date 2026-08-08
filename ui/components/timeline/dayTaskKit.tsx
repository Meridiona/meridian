//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Shared presentation kit for the day-task timeline feature — the ONE place the
// workstream palette and the reusable list/link atoms live, so the timeline card
// (DayTaskColumn) and the right-panel detail (DayTaskDetailPanel) render from the
// same source instead of each re-implementing bullets, hues, and chips with
// drifting inline styles. Pure presentation: no hooks, no data fetching, no
// layout math (that stays in dayTaskLayout.ts) and no worklog state (that lives
// in useWorklog.ts).


// ── Palette ──────────────────────────────────────────────────────────────────
// A stable-ish hue per task so workstreams read as distinct, both as the card's
// accent rail and as the detail panel's accent. Threaded as a prop from the card
// (which owns task order) into the panel, so a given task is always the same hue.
export const TASK_HUES = [
  'var(--t-accent)',
  '#3B82F6',
  '#D946EF',
  '#8b7cf6',
  '#6366F1',
  '#64748B',
] as const

/** The workstream hue for task `id` (its "T<n>" index when parseable, else the
 *  render index), wrapped over the palette. The single source of this mapping. */
export function taskHue(id: string, idx: number): string {
  const n = parseInt(id.replace(/^T/, ''), 10)
  return TASK_HUES[(Number.isFinite(n) ? n - 1 : idx) % TASK_HUES.length] ?? TASK_HUES[0]
}

// ── Shared atoms ─────────────────────────────────────────────────────────────

/** A bulleted list — the one bullet renderer for every day-task list (the panel's
 *  "What was done", and a worklog draft's update sections). `accent` tints
 *  the bullet marker; `size` sets the text size; `clamp` truncates each line to a
 *  single row (the space-constrained card preview). Renders nothing when empty. */
export function Bullets({ items, accent = 'var(--t-faint)', size = 12.5, clamp = false }: {
  items: string[]
  accent?: string
  size?: number
  clamp?: boolean
}) {
  if (!items || items.length === 0) return null
  return (
    <ul className="space-y-1.5">
      {items.map((line, i) => (
        <li key={i} className="flex gap-2" style={{ color: 'var(--t-muted)', fontSize: size, fontWeight: 500, lineHeight: 1.5 }}>
          <span className="shrink-0" style={{ color: accent }}>·</span>
          <span className="flex-1 min-w-0"
            style={clamp ? { display: '-webkit-box', WebkitLineClamp: 1, WebkitBoxOrient: 'vertical', overflow: 'hidden' } : undefined}>
            {line}
          </span>
        </li>
      ))}
    </ul>
  )
}

/** A small labelled block — a faint kicker over its content. The standard way a
 *  day-task detail section (When / Decisions / Status) is titled. */
export function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div>
      <p className="mt-label mb-1" style={{ color: 'var(--t-faint)' }}>{label}</p>
      {children}
    </div>
  )
}

