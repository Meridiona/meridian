//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// ONE list of the day, not two.
//
// This screen used to carry a plan ledger and a workstream list, one under the
// other, and half the day appeared in both - a ticket in the top list and the work
// that advanced it in the bottom one, with nothing to say they were the same thing.
// The reader had to do the join themselves. Now there is a single list: every real
// stretch of work, each marked with whether it was on the plan, and after it the
// planned tickets that no work touched.
//
// THE JOIN IS THE LEDGER'S, NOT A GUESS. `PlanVerdict.day_task_ids` comes back from
// `day_evidence::adherence` naming exactly which workstreams a ticket's outcome was
// read off - a posted worklog's, or the ones the model cited. Matching titles here
// would be re-deciding, badly, a question already answered upstream.
//
// THE 30-MINUTE FLOOR applies to the work rows (`TASK_MIN_MINUTES`, the same one
// that keeps `task_count` honest). A list including every five-minute glance is a
// list the reader sees through instantly. Nothing is hidden dishonestly: the tail is
// stated plainly underneath so the minutes still reconcile. Planned tickets are
// exempt - a plan is what someone committed to, not what cleared a bar.
//
// Clicking a work row opens the timeline's own DayTaskDetailPanel, so generate /
// approve / retarget / dismiss arrive with it and no worklog logic lives here.
// Clicking a planned-but-untouched row opens the ticket instead: there is no
// worklog to write for work that did not happen.

'use client'

import { motion, useReducedMotion } from 'framer-motion'
import { fmtDur } from '@/components/atoms'
import { TASK_MIN_MINUTES, type DayTask, type PlanVerdict } from '@/lib/api-types'

/** Split the day's tasks at the floor. Both halves are needed: one to list, one to
 *  account for. */
export function splitAtFloor(tasks: DayTask[]): { shown: DayTask[]; tailMinutes: number } {
  const shown = tasks
    .filter(t => t.minutes >= TASK_MIN_MINUTES)
    .slice()
    .sort((a, b) => b.minutes - a.minutes)
  const tailMinutes = tasks
    .filter(t => t.minutes < TASK_MIN_MINUTES)
    .reduce((n, t) => n + t.minutes, 0)
  return { shown, tailMinutes }
}

/** The left marker: a checkbox. Ticked means the work HAPPENED - this is
 *  deterministic, not a judgement. Any stretch of real tracked time appears here
 *  ticked, because you did in fact do it; a planned ticket nothing touched draws an
 *  empty box. No half-states: the box answers "did this happen", and the row's
 *  sub-line and time carry how much. */
function Checkbox({ done }: { done: boolean }) {
  if (!done) {
    return (
      <span
        className="shrink-0 rounded-md"
        style={{ width: 18, height: 18, border: '1.5px solid var(--t-faint-2)' }}
        aria-hidden
      />
    )
  }
  return (
    <span
      className="shrink-0 rounded-md flex items-center justify-center"
      style={{ width: 18, height: 18, background: 'var(--accent)' }}
      aria-hidden
    >
      <svg width="11" height="11" viewBox="0 0 12 12" fill="none">
        <path d="M2.5 6.2 4.8 8.5 9.5 3.5" stroke="#fff" strokeWidth="1.8"
          strokeLinecap="round" strokeLinejoin="round" />
      </svg>
    </span>
  )
}

/** One row, whatever it stands for. */
function Row({ title, sub, time, done, tint, chip, delay, onClick }: {
  title: string
  sub: string
  /** Already formatted, or a plain dash when there is no time to state. */
  time: string
  /** The work happened - ticks the box. */
  done: boolean
  tint?: boolean
  chip?: string
  delay: number
  onClick: () => void
}) {
  const reduce = useReducedMotion()
  return (
    <motion.li
      initial={reduce ? false : { opacity: 0, y: 4 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: reduce ? 0 : 0.3, delay: reduce ? 0 : delay }}
    >
      <button
        onClick={onClick}
        className="w-full flex items-center gap-3.5 text-left rounded-xl px-4 py-3 transition-opacity hover:opacity-80"
        style={{
          background: tint
            ? 'color-mix(in srgb, var(--accent) 8%, var(--t-box))'
            : 'var(--t-box)',
        }}
      >
        <Checkbox done={done} />

        <span className="flex-1 min-w-0">
          <span className="block truncate mt-body" style={{ color: 'var(--t-title)', fontWeight: 600 }}>
            {title}
          </span>
          {sub && (
            <span className="block truncate mt-body-sm" style={{ color: 'var(--t-faint-2)' }}>
              {sub}
            </span>
          )}
        </span>

        {chip && (
          <span
            className="shrink-0 mt-chip rounded-full px-2 py-1"
            style={{
              background: 'color-mix(in srgb, var(--accent) 14%, transparent)',
              color: 'var(--accent)',
            }}
          >
            {chip}
          </span>
        )}

        <span
          className="shrink-0 mt-body-sm tabular-nums text-right"
          style={{ color: 'var(--t-muted)', width: 56 }}
        >
          {time}
        </span>
        <span className="shrink-0 mt-body-sm" style={{ color: 'var(--t-faint-2)' }}>›</span>
      </button>
    </motion.li>
  )
}

export function WorkList({ tasks, plan, planned, onSelect, onOpenTask, delay = 0 }: {
  /** The day's tasks, unfiltered - the floor is applied here. */
  tasks: DayTask[]
  /** One verdict per planned ticket; empty on a day with no plan. */
  plan: PlanVerdict[]
  /** The day had a committed plan. Without one there is nothing to be on or off. */
  planned: boolean
  /** Called with the task and its index in the FULL day list, so the colour matches
   *  the timeline's. */
  onSelect: (task: DayTask, indexInDay: number) => void
  onOpenTask: (key: string, title?: string) => void
  delay?: number
}) {
  const { shown, tailMinutes } = splitAtFloor(tasks)

  // id → the ticket it advanced. Built from the ledger, so a row is on-plan only
  // when the ledger says which planned ticket it moved.
  const ticketOf = new Map<string, PlanVerdict>()
  for (const v of plan) {
    for (const id of v.day_task_ids) if (!ticketOf.has(id)) ticketOf.set(id, v)
  }

  // A planned ticket whose work is nowhere in the list above needs its own row -
  // otherwise a ticket the model called done while citing nothing simply vanishes,
  // which is the one thing a plan ledger must never do.
  const listed = new Set(shown.map(t => t.id))
  const missing = plan.filter(v => !v.day_task_ids.some(id => listed.has(id)))

  if (shown.length === 0 && missing.length === 0) return null

  return (
    <div className="flex flex-col">
      <ul className="flex flex-col gap-2">
        {shown.map((t, n) => {
          const indexInDay = tasks.indexOf(t)
          const ticket = ticketOf.get(t.id)
          return (
            <Row
              key={t.id}
              title={t.title}
              sub={
                !planned
                  ? ''
                  : ticket
                    ? `On today's plan · ${ticket.task_key}`
                    : 'Picked up along the way'
              }
              time={fmtDur(t.minutes * 60)}
              // Everything in this half of the list is real tracked work, so the
              // box is ticked. That is the deterministic fact - it happened.
              done
              chip={planned && !ticket ? 'extra' : undefined}
              delay={delay + 0.04 * n}
              onClick={() => onSelect(t, indexInDay)}
            />
          )
        })}

        {missing.map((v, n) => (
          <Row
            key={v.task_key}
            title={v.title || v.task_key}
            sub={
              v.evidence ||
              (v.outcome === 'not_touched'
                ? 'Planned - nothing tracked against it today'
                : 'No tracked time could be tied to it')
            }
            time={v.minutes > 0 ? fmtDur(v.minutes * 60) : '-'}
            tint
            done={false}
            delay={delay + 0.04 * (shown.length + n)}
            onClick={() => onOpenTask(v.task_key, v.title)}
          />
        ))}
      </ul>

      {/* The tail. Stated, never silently dropped - a list that quietly loses half
          an hour is a list nobody can reconcile with their own memory. */}
      {tailMinutes > 0 && (
        <p className="mt-body-sm px-4 pt-3" style={{ color: 'var(--t-faint-2)' }}>
          plus {fmtDur(tailMinutes * 60)} across shorter stretches
        </p>
      )}
    </div>
  )
}
