//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The day's work, in the walkthrough's two blocks: the plan, ticked off, and then
// what came up that nobody planned.
//
// KEYED BY TICKET, NOT BY WORKSTREAM. This is the load-bearing decision and it
// survived the restyle. On a planned day the top block is one row per COMMITTED
// TICKET, in plan order - so a `4 / 6` ring always shows six rows with four ticked,
// and the list reconciles with the number above it. An earlier version keyed the
// whole list by workstream and annotated each with the ticket it advanced; that
// silently dropped any ticket whose work was folded into a shared row (two tickets,
// one workstream) or had no tracked work at all, so a six-ticket plan could render
// four rows. Keying by ticket makes the plan portion count-exact by construction.
//
// A ticket's ROW still shows the WORK, not the ticket's own wording: the longest
// workstream behind it (`PlanVerdict.day_task_ids`, the join `day_evidence::
// adherence` already settled) supplies the title and the click-through to the
// worklog flow. Only a ticket with no tied workstream falls back to its own name and
// opens the ticket instead - there is no worklog to write for work that did not
// happen.
//
// EVERY ROW WITH WORK BEHIND IT OPENS THAT WORK'S DRAFT. This paragraph described
// the behaviour for a while before the code did: the rows were built with no click
// handler at all, so the finished half of the day - the rows whose worklogs are
// waiting to be edited, approved and posted - did nothing when clicked, while the
// untouched tickets, which have nothing to file, were the only ones that led
// anywhere. The draft flow itself was never missing; `SummaryTaskView` has always
// had generate / edit / approve / retarget / post, reached from the off-plan rows
// through the same `onSelect`. The plan block simply had no entry point into it.
// `drafts` reaches these rows for the same reason, so DRAFT READY TO POST shows on a
// committed ticket and not only on unplanned work. See
// `__tests__/summary-plan-row-drafts.test.ts`.
//
// WHY OFF-PLAN WORK GETS ITS OWN HEADED BLOCK rather than an inline chip on a row.
// A real day contains work nobody planned, and a summary that quietly folds it into
// the plan is the one people stop trusting - it is precisely the work they get asked
// about and cannot account for. Under its own heading it is also the only honest
// place to show the second worklog outcome: these strands matched no planned ticket,
// so Meridian drafts a NEW one rather than forcing them onto the nearest half-match.
// That is what the sub-line on each row says. (Being clickable is no longer what
// distinguishes them - both blocks now open a draft; the heading is.)
//
// NO SUB-FLOOR TAIL. The 30-minute floor (`TASK_MIN_MINUTES`) still decides what is
// a thing you did rather than a detour, but the remainder is no longer stated as a
// line under the list - the minutes therefore do not add up to the "time logged"
// figure above, deliberately, and the timeline is where the whole day is accounted
// for.
//
// # Who calls this
// [`DaySummaryOverlay`], in the left column of the plan row.
//
// # Related
// - `ui/components/tutorial/TutorialSummaryCard.tsx` — the scripted version of these
//   two blocks, and where the shape came from.
// - `./DayScore.tsx` — the same plan, as the donut above.

'use client'

import { motion, useReducedMotion } from 'framer-motion'
import { fmtDur } from '@/components/atoms'
import { TASK_MIN_MINUTES, type DayTask, type PlanVerdict } from '@/lib/api-types'

/** The day's substantial work, longest first. The floor keeps a five-minute glance
 *  out of a list whose claim is that everything in it is a thing you did. */
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

/** The workstream a planned ticket's row stands for, or `undefined` when the ticket
 *  has no tracked work at all.
 *
 *  A `PlanVerdict` can name SEVERAL workstreams (`day_task_ids`) - one ticket advanced
 *  by more than one strand of the day - while a worklog draft belongs to a single
 *  strand. The row therefore has to choose, and it chooses the LONGEST: that is the
 *  strand the row's own displayed duration mostly consists of, so its draft is the one
 *  a reader expects to open. Picking the first id instead would hand the click to
 *  whichever strand the adherence join happened to emit first, which is arbitrary and
 *  silently so.
 *
 *  Ids with no matching task are skipped rather than treated as empty - a verdict can
 *  reference a strand that fell below the day's floor and is not in `tasks` at all.
 *
 *  Exported for the test that pins the longest-wins rule; nothing else calls it. */
export function pickPrimary(
  v: Pick<PlanVerdict, 'day_task_ids'>,
  byId: Map<string, DayTask>,
): DayTask | undefined {
  let best: DayTask | undefined
  for (const id of v.day_task_ids) {
    const t = byId.get(id)
    if (t && (!best || t.minutes > best.minutes)) best = t
  }
  return best
}

/** A block heading inside the list panel. */
function BlockLabel({ children }: { children: React.ReactNode }) {
  return (
    <p className="mt-label mb-2" style={{ color: 'var(--t-faint-2)' }}>{children}</p>
  )
}

/** One planned ticket.
 *
 *  EVERY row with work behind it opens that work's worklog draft; a ticket nobody
 *  touched opens the ticket instead. Both are buttons - the only rows that are not
 *  are the ones with genuinely nowhere to go.
 *
 *  This used to be the opposite ("the plan rows are read, not opened"), which put the
 *  finished half of the day out of reach: a row reading `Done · 1h 34m` is exactly the
 *  work whose worklog is waiting to be edited, approved and posted, and clicking it
 *  did nothing at all. The rest of the file already described the behaviour restored
 *  here - the header has said the row supplies "the click-through to the worklog flow"
 *  since this component was written - so the code was the half that disagreed.
 *
 *  `badge` reaches the plan rows for the same reason: `DRAFT READY TO POST` on the
 *  ticket you committed to is the single most actionable thing in the panel, and it
 *  was rendering only under "not on the plan". */
function PlanRow({ title, sub, ticket, done, delay, onClick, badge }: {
  title: string
  sub: string
  ticket: string
  done: boolean
  delay: number
  onClick?: () => void
  badge?: DraftBadge
}) {
  const reduce = useReducedMotion()
  const chip = draftBadge(badge)
  const stale = staleNote(badge)
  // Same rule as `OffPlanRow`: only a draft that is an ASK tints its row. A ticket
  // that is done and filed is a receipt, and shouting at someone about work they
  // have already posted is how a badge stops being read.
  const waiting = !!chip?.loud
  const body = (
    <>
      <span
        className="inline-flex items-center justify-center shrink-0 rounded-md"
        style={{
          width: 17, height: 17, fontSize: 11, fontWeight: 800,
          color: done ? '#fff' : 'transparent',
          background: done ? 'var(--accent)' : 'transparent',
          border: done ? 'none' : '1.5px solid var(--t-ctrl-border)',
        }}
        aria-hidden
      >
        ✓
      </span>
      <span className="min-w-0 flex-1">
        <span
          className="mt-body-sm block truncate"
          style={{
            color: done ? 'var(--t-faint-2)' : 'var(--t-title)',
            textDecoration: done ? 'line-through' : 'none',
          }}
        >
          {title}
        </span>
        <span className="flex items-center gap-1.5 flex-wrap" style={{ fontSize: 11, color: 'var(--t-faint)' }}>
          <span className="truncate">{sub}</span>
          {chip && (
            <>
              <span aria-hidden>·</span>
              <span
                className="px-1.5 py-px rounded"
                style={{
                  fontSize: 9.5, fontWeight: 800, letterSpacing: '0.06em',
                  color: chip.tone,
                  background: `color-mix(in srgb, ${chip.tone} 15%, transparent)`,
                }}
              >
                {chip.label}
              </span>
            </>
          )}
          {stale && <><span aria-hidden>·</span><span>{stale}</span></>}
        </span>
      </span>
      <span className="shrink-0" style={{ fontSize: 11, color: 'var(--t-faint)' }}>{ticket}</span>
    </>
  )
  return (
    <motion.li
      initial={reduce ? false : { opacity: 0, y: 4 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: reduce ? 0 : 0.3, delay: reduce ? 0 : delay }}
    >
      {onClick ? (
        <button
          onClick={onClick}
          className="w-full flex items-center gap-2.5 rounded-lg px-2 py-2 text-left transition-shadow hover:shadow-[0_1px_6px_-2px_rgba(0,0,0,0.18)]"
          style={{
            cursor: 'pointer',
            background: waiting
              ? `color-mix(in srgb, ${chip.tone} 7%, transparent)`
              : 'transparent',
            border: waiting
              ? `1px solid color-mix(in srgb, ${chip.tone} 26%, transparent)`
              : '1px solid transparent',
          }}
        >
          {body}
        </button>
      ) : (
        <div
          className="flex items-center gap-2.5 rounded-lg px-2 py-2 text-left"
          style={{ border: '1px solid transparent' }}
        >
          {body}
        </div>
      )}
    </motion.li>
  )
}

/** How a row's worklog stands. `undefined` = we have no draft row for this task. */
export type DraftBadge = { state: string; stale_minutes: number | null; stale: boolean } | undefined

/** The badge for a row's worklog state, or `null` when there is nothing to say.
 *
 *  THIS USED TO BE A FIXED STRING. Every off-plan row read "no ticket yet - draft
 *  ready", in the same faint grey as the duration beside it, whether or not a draft
 *  existed and whether or not it had already been posted. Three different situations,
 *  one sentence, styled to be ignored - so the one thing worth acting on looked
 *  identical to the two things that needed nothing.
 *
 *  Only `drafted` is an ASK, so only `drafted` gets the loud treatment. A posted row
 *  is a receipt and a quiet one is correct; shouting at someone about work they have
 *  already filed is how a badge stops being read at all. */
function draftBadge(badge: DraftBadge): { label: string; tone: string; loud: boolean } | null {
  if (!badge) return null
  switch (badge.state) {
    case 'drafted':
      return { label: 'DRAFT READY TO POST', tone: 'var(--t-accent)', loud: true }
    case 'approved':
      return { label: 'APPROVED', tone: 'var(--color-state-approved)', loud: false }
    case 'posted':
      return { label: 'POSTED', tone: 'var(--color-state-approved)', loud: false }
    case 'error':
      return { label: 'POST FAILED', tone: 'var(--status-error-dot)', loud: true }
    default:
      return null
  }
}

/** "· 40m of work since" - the draft's age, in the only unit that matters here.
 *
 *  Wall-clock age would be the obvious choice and the wrong one: a draft written at
 *  09:00 and looked at after lunch is not stale if nothing happened on that task in
 *  between. What makes a draft out of date is WORK it does not describe, which is
 *  exactly `stale_minutes`.
 *
 *  WHETHER that gap is worth mentioning is NOT decided here. It is decided once, in
 *  Rust, by `WORKLOG_STALE_MINUTES` - and `stale` is that decision, already made. This
 *  used to re-threshold `stale_minutes` against a local 25, against the daemon's 15,
 *  which put the screen and the notification into direct contradiction over the same
 *  draft: between 15 and 25 minutes the user got a toast saying their draft was out of
 *  date, opened the summary the toast linked to, and found the row saying nothing was
 *  wrong. Two definitions of one word cannot both be right, and the one the user was
 *  already told is the one that has to win. `api-types.ts` says exactly this on the
 *  field itself. */
function staleNote(badge: DraftBadge): string | null {
  if (!badge || badge.state !== 'drafted' || !badge.stale) return null
  const m = badge.stale_minutes ?? 0
  if (m <= 0) return null
  return `${fmtDur(m * 60)} of work since`
}

/** One strand of work no planned ticket claims. Leads with the time it took rather
 *  than a ticket key, because the point of the row is that it does not have one. */
function OffPlanRow({ title, minutes, delay, onClick, badge }: {
  title: string
  minutes: number
  delay: number
  onClick: () => void
  badge: DraftBadge
}) {
  const reduce = useReducedMotion()
  const chip = draftBadge(badge)
  const stale = staleNote(badge)
  // The row itself carries the state when there is something to do. A chip alone is a
  // 9px detail in a list read at a glance - the tinted surface is what makes "three
  // rows, all waiting on you" legible without reading a word.
  const waiting = !!chip?.loud
  return (
    <motion.li
      initial={reduce ? false : { opacity: 0, y: 4 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: reduce ? 0 : 0.3, delay: reduce ? 0 : delay }}
    >
      <button
        onClick={onClick}
        className="w-full flex items-center gap-2.5 rounded-lg px-2 py-2 text-left transition-shadow hover:shadow-[0_1px_6px_-2px_rgba(0,0,0,0.18)]"
        style={{
          cursor: 'pointer',
          background: waiting
            ? `color-mix(in srgb, ${chip.tone} 7%, transparent)`
            : 'transparent',
          border: waiting
            ? `1px solid color-mix(in srgb, ${chip.tone} 26%, transparent)`
            : '1px solid transparent',
        }}
      >
        <span
          className="inline-flex items-center justify-center shrink-0 rounded-md"
          style={{
            width: 17, height: 17, fontSize: 11, fontWeight: 800,
            color: 'var(--color-state-pending)',
            background: 'color-mix(in srgb, var(--color-state-pending) 16%, transparent)',
          }}
          aria-hidden
        >
          ↯
        </span>
        <span className="min-w-0 flex-1">
          <span className="mt-body-sm block truncate" style={{ color: 'var(--t-title)' }}>{title}</span>
          <span className="flex items-center gap-1.5 flex-wrap" style={{ fontSize: 11, color: 'var(--t-faint)' }}>
            <span>{fmtDur(minutes * 60)}</span>
            {chip && (
              <>
                <span aria-hidden>·</span>
                <span
                  className="px-1.5 py-px rounded"
                  style={{
                    fontSize: 9.5, fontWeight: 800, letterSpacing: '0.06em',
                    color: chip.tone,
                    background: `color-mix(in srgb, ${chip.tone} 15%, transparent)`,
                  }}
                >
                  {chip.label}
                </span>
              </>
            )}
            {stale && <><span aria-hidden>·</span><span>{stale}</span></>}
          </span>
        </span>
        <span className="shrink-0" style={{ fontSize: 14, color: waiting ? chip.tone : 'var(--t-faint)' }}>›</span>
      </button>
    </motion.li>
  )
}

export function WorkList({ tasks, plan, planned, onSelect, onOpenTask, drafts, delay = 0 }: {
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
  /** Worklog state per task id, from `get_day_draft_states`. Absent while it loads,
   *  and a task with no entry has no draft - both render without a badge, which is
   *  the honest answer in each case. */
  drafts?: Map<string, { state: string; stale_minutes: number | null; stale: boolean }>
  delay?: number
}) {
  const byId = new Map(tasks.map(t => [t.id, t]))

  // ── No plan: one block of the day's real work, every row openable. There is no
  //    plan to be off, so nothing is headed "not on the plan" - the whole list IS
  //    the day. A day nobody planned did not fail an exercise. ──────────────────
  if (!planned) {
    const { shown } = splitAtFloor(tasks)
    if (shown.length === 0) return null
    return (
      <Panel>
        <BlockLabel>WHAT YOU WORKED ON</BlockLabel>
        <ul className="flex flex-col gap-1">
          {shown.map((t, n) => (
            <OffPlanRow
              key={t.id}
              title={t.title}
              minutes={t.minutes}
              delay={delay + 0.04 * n}
              onClick={() => onSelect(t, tasks.indexOf(t))}
              badge={drafts?.get(t.id)}
            />
          ))}
        </ul>
      </Panel>
    )
  }

  // ── Planned: the plan first, keyed by ticket, then what came up on top of it.
  const consumed = new Set<string>()
  for (const v of plan) for (const id of v.day_task_ids) if (byId.has(id)) consumed.add(id)

  // The floor applies to the off-plan half only; a ticket you committed to shows
  // however little it took.
  const offPlan = tasks
    .filter(t => t.minutes >= TASK_MIN_MINUTES && !consumed.has(t.id))
    .sort((a, b) => b.minutes - a.minutes)

  return (
    <Panel>
      <BlockLabel>TODAY&apos;S PLAN</BlockLabel>
      <ul className="flex flex-col gap-1">
        {plan.map((v, n) => {
          // The workstream this row stands for. Its title is what the person actually
          // did, which reads better than the ticket's own name, and it is what the row
          // opens. A ticket with no tied workstream (not touched, or closed with
          // nothing logged) falls back to its own title and opens the ticket instead.
          //
          // WHICH workstream, when a ticket has several: the LONGEST. `day_task_ids`
          // is a list - one ticket can be advanced by several strands - and drafts are
          // per strand, so a row that opens "the first one" opens whichever the join
          // happened to emit first. The biggest strand is the one whose draft the row's
          // own duration mostly describes, so it is the honest single destination.
          // Reaching the smaller strands is what the timeline is for; a row that opens
          // the wrong draft would be worse than today's row that opens nothing.
          const primary = pickPrimary(v, byId)
          const done = v.outcome === 'done'
          return (
            <PlanRow
              key={v.task_key}
              title={primary ? primary.title : v.title || v.task_key}
              sub={
                primary
                  ? done
                    ? `Done · ${fmtDur(v.minutes * 60)}`
                    : v.outcome === 'partial'
                      ? `In progress · ${fmtDur(v.minutes * 60)}`
                      : 'Carried over to tomorrow'
                  : v.outcome === 'not_touched'
                    ? 'Carried over to tomorrow'
                    : v.evidence || 'No tracked time could be tied to it'
              }
              ticket={v.task_key}
              done={done}
              delay={delay + 0.04 * n}
              badge={primary ? drafts?.get(primary.id) : undefined}
              // Work behind it -> that work's worklog draft, the same destination and
              // the same `onSelect` the off-plan rows use. Nothing behind it -> the
              // ticket, because there is no worklog to write for work that did not
              // happen.
              onClick={
                primary
                  ? () => onSelect(primary, tasks.indexOf(primary))
                  : () => onOpenTask(v.task_key, v.title)
              }
            />
          )
        })}
      </ul>

      {offPlan.length > 0 && (
        <div className="mt-3.5 pt-3 border-t" style={{ borderColor: 'var(--t-card-border)' }}>
          <BlockLabel>CAME UP TODAY - NOT ON THE PLAN</BlockLabel>
          <ul className="flex flex-col gap-1">
            {offPlan.map((t, n) => (
              <OffPlanRow
                key={t.id}
                title={t.title}
                minutes={t.minutes}
                delay={delay + 0.04 * (plan.length + n)}
                onClick={() => onSelect(t, tasks.indexOf(t))}
                badge={drafts?.get(t.id)}
              />
            ))}
          </ul>
        </div>
      )}
    </Panel>
  )
}

/** The quiet panel both blocks sit on. A row of small type with a marker and a time
 *  on it needs an edge to sit against, or it floats with nothing holding it. */
function Panel({ children }: { children: React.ReactNode }) {
  return (
    <div
      className="rounded-xl p-3.5 h-full"
      style={{ background: 'var(--t-box)', border: '1px solid var(--t-card-border)' }}
    >
      {children}
    </div>
  )
}
