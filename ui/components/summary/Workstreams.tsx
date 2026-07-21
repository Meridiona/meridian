//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Everything you worked on - and the way into a worklog for any of it.
//
// THE 30-MINUTE FLOOR. Only workstreams of at least TASK_MIN_MINUTES appear. A
// list that includes every five-minute glance is a list the reader sees through
// instantly, and it is the same rule that already keeps `task_count` honest
// (`meridian_core::day_evidence::TASK_MIN_MINUTES`). What falls below the floor is
// not hidden dishonestly: the tail is stated plainly underneath, so the minutes
// still add up.
//
// Rows, not the pill row this replaces. Pills gave every workstream the same
// visual weight whatever its size, which made a seven-hour thread and a
// half-hour detour look like peers. A row can carry a proportional bar, its
// measured time, and its worklog state, which is what a person is actually
// scanning for.
//
// Clicking one opens the timeline's own DayTaskDetailPanel, so generate / approve
// / retarget / dismiss all arrive with it and no worklog logic lives here.

'use client'

import { motion, useReducedMotion } from 'framer-motion'
import { fmtDur } from '@/components/atoms'
import { taskHue } from '@/components/timeline/dayTaskKit'
import { TASK_MIN_MINUTES, type DayTask } from '@/lib/api-types'

/** Split the day's tasks at the floor. Both halves are needed: one to list, one
 *  to account for. */
export function splitAtFloor(tasks: DayTask[]): { shown: DayTask[]; tailMinutes: number } {
  const shown = tasks.filter(t => t.minutes >= TASK_MIN_MINUTES)
  const tailMinutes = tasks
    .filter(t => t.minutes < TASK_MIN_MINUTES)
    .reduce((n, t) => n + t.minutes, 0)
  return { shown, tailMinutes }
}

export function Workstreams({
  tasks,
  onSelect,
  delay = 0,
}: {
  /** The day's tasks, unfiltered - the floor is applied here. */
  tasks: DayTask[]
  /** Called with the task and its index in the FULL day list, so the colour
   *  matches the timeline's. */
  onSelect: (task: DayTask, indexInDay: number) => void
  delay?: number
}) {
  const reduce = useReducedMotion()
  const { shown, tailMinutes } = splitAtFloor(tasks)
  if (shown.length === 0) return null

  const longest = Math.max(...shown.map(t => t.minutes), 1)

  return (
    // No heading here: the summary's `Section` owns the label, the spacing and the
    // frame together, so one thing decides how a block is introduced.
    <div className="flex flex-col">
      <ul className="flex flex-col">
        {shown.map((t, n) => {
          const indexInDay = tasks.indexOf(t)
          const hue = taskHue(t.id, indexInDay)
          return (
            <motion.li
              key={t.id}
              initial={reduce ? false : { opacity: 0, y: 4 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ duration: reduce ? 0 : 0.3, delay: reduce ? 0 : delay + 0.04 * n }}
            >
              <button
                onClick={() => onSelect(t, indexInDay)}
                className="w-full flex items-center gap-3 text-left rounded-lg px-2 py-2 transition-colors hover:bg-[var(--t-row-hover)]"
              >
                <span
                  className="shrink-0 rounded-full"
                  style={{ width: 7, height: 7, background: hue }}
                />
                <span
                  className="flex-1 min-w-0 truncate mt-body"
                  style={{ color: 'var(--t-title)', fontWeight: 600 }}
                >
                  {t.title}
                </span>

                {/* Proportional to the longest thread of the day, not to a clock.
                    The bar is for comparison between rows; the exact figure is the
                    number beside it. */}
                <span
                  className="hidden sm:block shrink-0 rounded-full overflow-hidden"
                  style={{ width: 88, height: 5, background: 'var(--t-track)' }}
                >
                  <span
                    className="block h-full rounded-full"
                    style={{ width: `${(t.minutes / longest) * 100}%`, background: hue }}
                  />
                </span>

                <span
                  className="shrink-0 mt-body-sm tabular-nums text-right"
                  style={{ color: 'var(--t-muted)', width: 56 }}
                >
                  {fmtDur(t.minutes * 60)}
                </span>

                <WorklogChip task={t} />
              </button>
            </motion.li>
          )
        })}
      </ul>

      {/* The tail. Stated, never silently dropped - a list that quietly loses
          half an hour is a list nobody can reconcile with their own memory. */}
      {tailMinutes > 0 && (
        <p className="mt-body-sm px-2 pt-2" style={{ color: 'var(--t-faint-2)' }}>
          plus {fmtDur(tailMinutes * 60)} across shorter stretches
        </p>
      )}
    </div>
  )
}

/** Where this workstream's worklog stands, in one word. Nothing when there is
 *  nothing to say - an "unlogged" chip on every row would read as a chore list. */
function WorklogChip({ task }: { task: DayTask }) {
  if (!task.posted_target_key) return <span className="shrink-0" style={{ width: 66 }} />
  return (
    <span
      className="shrink-0 mt-chip rounded-full px-2 py-1 text-center truncate"
      style={{
        width: 66,
        background: 'color-mix(in srgb, var(--accent) 10%, transparent)',
        color: 'var(--accent)',
      }}
      title={`Posted to ${task.posted_target_key}`}
    >
      posted
    </span>
  )
}
