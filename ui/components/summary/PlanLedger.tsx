//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The plan, as rows. The same array the ring draws, in the same order.
//
// Order is the PLAN's order, not sorted by outcome. Sorting the untouched to the
// bottom would rank the tickets by how well they went, which is a scoreboard move;
// keeping the morning's order means the list reads back as the thing they wrote.
//
// Outcome is carried by a glyph and by weight, never by hue - see AchievementRing
// for why there are no traffic lights on this screen.
//
// A `certain` verdict came out of the DATABASE (a posted worklog, a linked ticket,
// a closed ticket) rather than out of the model, and says so, quietly. That mark is
// the difference between "we think you did this" and "you did this", and a person
// checking the number deserves to see which is which.
//
// # Related
// - `AchievementRing` - the same verdicts as arcs; hover is shared through
//   `activeIndex`/`onHover`.

'use client'

import { motion, useReducedMotion } from 'framer-motion'
import { fmtDur } from '@/components/atoms'
import type { PlanVerdict } from '@/lib/api-types'

/** The mark for one outcome. A ring rather than a cross for untouched: a cross
 *  reads as wrong, and a ticket you did not get to is not wrong. */
function Mark({ outcome }: { outcome: PlanVerdict['outcome'] }) {
  const common = { width: 15, height: 15, viewBox: '0 0 15 15', fill: 'none' } as const
  if (outcome === 'done') {
    return (
      <svg {...common} aria-hidden>
        <circle cx="7.5" cy="7.5" r="7.5" fill="var(--accent)" />
        <path
          d="M4.2 7.7l2.4 2.3 4.2-4.6"
          stroke="#fff"
          strokeWidth="1.7"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
      </svg>
    )
  }
  if (outcome === 'partial') {
    return (
      <svg {...common} aria-hidden>
        <circle cx="7.5" cy="7.5" r="6.7" stroke="var(--accent)" strokeWidth="1.6" />
        {/* Half filled, literally: the work is half done. */}
        <path d="M7.5 1.6a5.9 5.9 0 010 11.8z" fill="var(--accent)" opacity="0.6" />
      </svg>
    )
  }
  return (
    <svg {...common} aria-hidden>
      <circle cx="7.5" cy="7.5" r="6.7" stroke="var(--t-hair)" strokeWidth="1.6" />
    </svg>
  )
}

export function PlanLedger({
  plan,
  activeIndex,
  onHover,
  onOpenTask,
}: {
  plan: PlanVerdict[]
  activeIndex: number | null
  onHover: (index: number | null) => void
  /** Open the ticket's own detail. Same handler the timeline uses. */
  onOpenTask: (key: string, title?: string) => void
}) {
  const reduce = useReducedMotion()

  return (
    <ul className="flex flex-col">
      {plan.map((v, i) => {
        const untouched = v.outcome === 'not_touched'
        return (
          <motion.li
            key={v.task_key}
            initial={reduce ? false : { opacity: 0, y: 4 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ duration: reduce ? 0 : 0.3, delay: reduce ? 0 : 0.3 + 0.04 * i }}
            onMouseEnter={() => onHover(i)}
            onMouseLeave={() => onHover(null)}
          >
            <button
              onClick={() => onOpenTask(v.task_key, v.title)}
              className="w-full flex items-baseline gap-3 text-left rounded-lg px-2 py-2 transition-colors"
              style={{
                background: activeIndex === i ? 'var(--t-row-hover)' : 'transparent',
                // An untouched ticket recedes rather than disappearing: it was
                // still part of the day's intent, and hiding it would make the
                // ring's empty arcs unexplainable.
                opacity: untouched ? 0.62 : 1,
              }}
            >
              <span className="shrink-0 self-center">
                <Mark outcome={v.outcome} />
              </span>

              <span
                className="shrink-0 mt-chip self-center"
                style={{ color: 'var(--t-faint-2)', minWidth: 62 }}
              >
                {v.task_key}
              </span>

              <span
                className="flex-1 min-w-0 truncate mt-body"
                style={{ color: 'var(--t-title)', fontWeight: untouched ? 500 : 600 }}
              >
                {v.title}
              </span>

              {v.evidence && (
                <span
                  className="hidden md:block shrink-0 truncate mt-body-sm"
                  style={{ color: 'var(--t-faint-2)', maxWidth: 230 }}
                  title={v.evidence}
                >
                  {v.evidence}
                </span>
              )}

              <span
                className="shrink-0 mt-body-sm tabular-nums text-right"
                style={{ color: v.minutes > 0 ? 'var(--t-muted)' : 'var(--t-faint-2)', width: 56 }}
              >
                {v.minutes > 0 ? fmtDur(v.minutes * 60) : '-'}
              </span>

              {/* The confidence mark. A filled dot means the database settled it;
                  a hollow one means the model read the day and judged. */}
              <span
                className="shrink-0 rounded-full self-center"
                title={v.certain ? 'Confirmed from your tracker' : 'Judged from the day’s work'}
                style={{
                  width: 5,
                  height: 5,
                  background: v.certain ? 'var(--accent)' : 'transparent',
                  border: v.certain ? 'none' : '1px solid var(--t-hair)',
                }}
              />
            </button>
          </motion.li>
        )
      })}
    </ul>
  )
}
