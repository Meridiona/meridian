//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The hero for a day with NO plan: what the day turned out to be about.
//
// No fake ring, no target invented after the fact, no nag. Somebody who did not
// plan their morning did not fail an exercise, and a screen that greets them with
// an empty progress meter is a screen that punishes them for not using a feature.
//
// The same idiom as AchievementRing, rotated flat: one band, divided by theme, in
// proportion to real measured minutes. Where the ring lets you count your plan,
// this lets you see at a glance whether the day was one thing or five.
//
// THE MODEL NAMES THE THEMES; THE MINUTES ARE OURS. `theme.day_task_ids` points at
// real workstreams and the widths come from their measured time - the model is
// never asked for a duration, so it cannot put a number on this screen that the
// timeline disagrees with.
//
// # Related
// - `AchievementRing` - the planned-day counterpart.
// - `Workstreams` - the same work as an itemised list underneath.

'use client'

import { motion, useReducedMotion } from 'framer-motion'
import { fmtDur } from '@/components/atoms'
import { taskHue } from '@/components/timeline/dayTaskKit'
import type { DayTask, DayTheme } from '@/lib/api-types'

/** A theme with its real minutes resolved from the workstreams it points at. */
interface Resolved {
  title: string
  minutes: number
  hue: string
}

/** Resolve each theme's measured time, dropping any that resolved to nothing.
 *
 *  `hue` comes from the theme's FIRST workstream, using the timeline's own
 *  palette, so a band here is the same colour as the card that work has over
 *  there. Two screens giving the same work two colours is how a person stops
 *  believing they are looking at the same thing. */
export function resolveThemes(themes: DayTheme[], tasks: DayTask[]): Resolved[] {
  const index = new Map(tasks.map((t, i) => [t.id, { task: t, i }]))
  return themes
    .map(theme => {
      const members = theme.day_task_ids.map(id => index.get(id)).filter(Boolean) as {
        task: DayTask
        i: number
      }[]
      return {
        title: theme.title,
        minutes: members.reduce((n, m) => n + m.task.minutes, 0),
        hue: members.length ? taskHue(members[0].task.id, members[0].i) : 'var(--accent)',
      }
    })
    .filter(t => t.minutes > 0)
    .sort((a, b) => b.minutes - a.minutes)
}

export function DayShape({
  themes,
  tasks,
  focusSeconds,
  switchCount,
}: {
  themes: DayTheme[]
  tasks: DayTask[]
  focusSeconds: number
  switchCount: number
}) {
  const reduce = useReducedMotion()
  const rows = resolveThemes(themes, tasks)
  const total = rows.reduce((n, r) => n + r.minutes, 0)
  if (rows.length === 0 || total === 0) return null

  return (
    <div className="w-full">
      <p className="mt-label mb-3" style={{ color: 'var(--t-faint-2)' }}>
        What the day was about
      </p>

      {/* The band. One bar, split - not a bar chart with an axis. There is no
          scale to read off it, only a proportion to see. */}
      <div className="flex gap-1 w-full overflow-hidden" style={{ height: 10 }}>
        {rows.map((r, i) => (
          <motion.div
            key={r.title}
            className="rounded-full"
            style={{ background: r.hue, flexBasis: `${(r.minutes / total) * 100}%` }}
            initial={reduce ? false : { scaleX: 0, originX: 0 }}
            animate={{ scaleX: 1 }}
            transition={{ duration: reduce ? 0 : 0.5, delay: reduce ? 0 : 0.06 * i, ease: [0.16, 1, 0.3, 1] }}
          />
        ))}
      </div>

      <ul className="flex flex-col gap-1.5 mt-4">
        {rows.map((r, i) => (
          <motion.li
            key={r.title}
            className="flex items-baseline gap-3"
            initial={reduce ? false : { opacity: 0, y: 4 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ duration: reduce ? 0 : 0.3, delay: reduce ? 0 : 0.2 + 0.05 * i }}
          >
            <span
              className="shrink-0 rounded-full self-center"
              style={{ width: 7, height: 7, background: r.hue }}
            />
            <span className="flex-1 min-w-0 truncate mt-body" style={{ color: 'var(--t-title)', fontWeight: 600 }}>
              {r.title}
            </span>
            <span className="shrink-0 mt-body-sm tabular-nums" style={{ color: 'var(--t-muted)' }}>
              {fmtDur(r.minutes * 60)}
            </span>
          </motion.li>
        ))}
      </ul>

      <p className="mt-body-sm mt-4" style={{ color: 'var(--t-faint-2)' }}>
        {fmtDur(focusSeconds)} focused
        {switchCount > 0 && ` · ${switchCount} switches`}
      </p>
    </div>
  )
}
