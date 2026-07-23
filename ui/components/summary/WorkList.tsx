//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// ONE list of the day: the plan, then the work done on top of it.
//
// KEYED BY TICKET, NOT BY WORKSTREAM. This is the load-bearing decision. On a
// planned day the top of the list is one row per COMMITTED TICKET, in plan order -
// so a `4 / 6` ring always shows six rows with four ticked, and the list reconciles
// with the number above it. An earlier version keyed the whole list by workstream
// and annotated each with the ticket it advanced; that silently dropped any ticket
// whose work was folded into a shared row (two tickets, one workstream) or had no
// tracked work at all, so a six-ticket plan could render four rows. Keying by ticket
// makes the plan portion count-exact by construction.
//
// A ticket's ROW still shows the WORK, not the ticket's own wording: the first
// workstream behind it (`PlanVerdict.day_task_ids`, the join `day_evidence::
// adherence` already settled) supplies the title and the click-through to the
// worklog flow. Only a ticket with no tied workstream falls back to its own name and
// opens the ticket instead - there is no worklog to write for work that did not
// happen.
//
// Below the plan sits the BONUS half: substantial work no planned ticket claims.
// The 30-minute floor applies there (`TASK_MIN_MINUTES`, the same one that keeps
// `task_count` honest) and the sub-floor remainder is stated as a tail, so the
// minutes still reconcile. Planned tickets are exempt from the floor - a ticket you
// committed to shows however little it took.
//
// Clicking a row opens the timeline's own DayTaskDetailPanel (generate / approve /
// retarget / dismiss arrive with it, no worklog logic here) or, for an untouched
// ticket, the ticket itself.

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
            className="shrink-0 mt-chip rounded-full px-2 py-1 whitespace-nowrap"
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

export function WorkList({ tasks, plan, planned, allPlanDone, onSelect, onOpenTask, delay = 0 }: {
  /** The day's tasks, unfiltered - the floor is applied here. */
  tasks: DayTask[]
  /** One verdict per planned ticket; empty on a day with no plan. */
  plan: PlanVerdict[]
  /** The day had a committed plan. Without one there is nothing to be on or off. */
  planned: boolean
  /** Every planned ticket came out done. Only then is unplanned work honestly
   *  "over-delivered" - you cleared the whole plan AND did more. Short of that,
   *  unplanned work is still credited, just not as beating a plan you did not
   *  finish. */
  allPlanDone: boolean
  /** Called with the task and its index in the FULL day list, so the colour matches
   *  the timeline's. */
  onSelect: (task: DayTask, indexInDay: number) => void
  onOpenTask: (key: string, title?: string) => void
  delay?: number
}) {
  const byId = new Map(tasks.map(t => [t.id, t]))

  // ── No plan: the list is just the day's real work, every row ticked (it
  //    happened), no plan annotations to make. ─────────────────────────────────
  if (!planned) {
    const { shown, tailMinutes } = splitAtFloor(tasks)
    if (shown.length === 0) return null
    return (
      <div className="flex flex-col">
        <ul className="flex flex-col gap-2">
          {shown.map((t, n) => (
            <Row
              key={t.id}
              title={t.title}
              sub=""
              time={fmtDur(t.minutes * 60)}
              done
              delay={delay + 0.04 * n}
              onClick={() => onSelect(t, tasks.indexOf(t))}
            />
          ))}
        </ul>
        <Tail minutes={tailMinutes} />
      </div>
    )
  }

  // ── Planned: the list is keyed by PLANNED TICKET first, then the work done on
  //    top of the plan. Keying the plan portion by ticket (not by workstream) is
  //    what makes the rows reconcile with the ring: one row per committed ticket,
  //    so `4 / 6` always shows six rows with four ticked. Keying it by workstream
  //    silently dropped any ticket whose work was folded into a shared row.
  const consumed = new Set<string>()
  for (const v of plan) for (const id of v.day_task_ids) if (byId.has(id)) consumed.add(id)

  // The word on an unplanned-work row. "Over-delivered" is earned only by a clean
  // sweep of the plan; short of that the work is a genuine pickup, credited warmly
  // without claiming a plan was beaten.
  const bonusChip = allPlanDone ? 'over-delivered' : 'also got to'

  // The bonus half: substantial work no planned ticket claims. The floor applies
  // here (a five-minute glance is not a thing you "also did"); planned tickets are
  // exempt, because a ticket you committed to still shows however little it took.
  const bonus = tasks
    .filter(t => t.minutes >= TASK_MIN_MINUTES && !consumed.has(t.id))
    .sort((a, b) => b.minutes - a.minutes)
  const tailMinutes = tasks
    .filter(t => t.minutes < TASK_MIN_MINUTES && !consumed.has(t.id))
    .reduce((n, t) => n + t.minutes, 0)

  return (
    <div className="flex flex-col gap-4">
      <ul className="flex flex-col gap-2">
        {plan.map((v, n) => {
          // The first real workstream behind this ticket, if any. Its title is what
          // the person actually did, which reads better than the ticket's own name;
          // clicking it opens the worklog flow for that work. A ticket with no tied
          // workstream (not touched, or closed with nothing logged) falls back to
          // its own title and opens the ticket instead.
          const primary = v.day_task_ids.map(id => byId.get(id)).find(Boolean)
          const done = v.outcome === 'done'
          return (
            <Row
              key={v.task_key}
              title={primary ? primary.title : v.title || v.task_key}
              sub={
                primary
                  ? `On today's plan · ${v.task_key}${v.outcome === 'partial' ? ' · in progress' : ''}`
                  : v.evidence ||
                    (v.outcome === 'not_touched'
                      ? 'Planned - nothing tracked against it today'
                      : 'No tracked time could be tied to it')
              }
              time={v.minutes > 0 ? fmtDur(v.minutes * 60) : '-'}
              tint={!done}
              done={done}
              delay={delay + 0.04 * n}
              onClick={() =>
                primary ? onSelect(primary, tasks.indexOf(primary)) : onOpenTask(v.task_key, v.title)
              }
            />
          )
        })}

        {bonus.map((t, n) => (
          <Row
            key={t.id}
            title={t.title}
            sub="Picked up along the way"
            time={fmtDur(t.minutes * 60)}
            done
            chip={bonusChip}
            delay={delay + 0.04 * (plan.length + n)}
            onClick={() => onSelect(t, tasks.indexOf(t))}
          />
        ))}
      </ul>

      <Tail minutes={tailMinutes} />
    </div>
  )
}

/** The sub-floor remainder, stated plainly - a list that quietly loses half an hour
 *  is a list nobody can reconcile with their own memory. Renders nothing when there
 *  is nothing below the floor. */
function Tail({ minutes }: { minutes: number }) {
  if (minutes <= 0) return null
  return (
    <p className="mt-body-sm px-4" style={{ color: 'var(--t-faint-2)' }}>
      plus {fmtDur(minutes * 60)} across shorter stretches
    </p>
  )
}
