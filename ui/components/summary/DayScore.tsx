//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The day's numbers: how much of the plan came out, and the figures behind it.
//
// ONE DONUT, ONE ARC. This was a segmented ring - one arc per planned ticket, so
// three of five looked like three of five before a number was read - and it is now
// the single-arc donut the walkthrough shows, so the screen a new user is taught is
// the screen they get. The count it stops conveying at a glance is carried by the
// `done / planned` figure immediately beside it and by the checklist below, where
// every ticket has its own row.
//
// EVERY NUMBER HERE IS MEASURED. The counts and the percentage come from
// `day_evidence::adherence` in Rust; the durations are summed from the same day-task
// list the rows below render. The model contributes no figure.
//
// # Related
// - `WorkList` - the same plan and the same work, itemised. On a no-plan day this
//   component renders the stat row without the donut.

'use client'

import { motion, useReducedMotion } from 'framer-motion'
import { fmtDur } from '@/components/atoms'
import type { Adherence, PlanVerdict } from '@/lib/api-types'

const SIZE = 76
const STROKE = 8
const R = (SIZE - STROKE) / 2
const C = 2 * Math.PI * R

/** The plan, as one filled arc. */
function PlanDonut({ pct }: { pct: number }) {
  const reduce = useReducedMotion()
  const dash = (Math.max(0, Math.min(100, pct)) / 100) * C
  const track = 'color-mix(in srgb, var(--t-title) 11%, var(--t-track))'

  return (
    <div className="relative shrink-0" style={{ width: SIZE, height: SIZE }}>
      <svg width={SIZE} height={SIZE} viewBox={`0 0 ${SIZE} ${SIZE}`} aria-hidden>
        <circle cx={SIZE / 2} cy={SIZE / 2} r={R} fill="none" stroke={track} strokeWidth={STROKE} />
        <motion.circle
          cx={SIZE / 2} cy={SIZE / 2} r={R} fill="none"
          stroke="var(--accent)" strokeWidth={STROKE} strokeLinecap="round"
          strokeDasharray={`${dash} ${C - dash}`}
          transform={`rotate(-90 ${SIZE / 2} ${SIZE / 2})`}
          initial={reduce ? false : { opacity: 0 }}
          animate={{ opacity: 1 }}
          transition={{ duration: reduce ? 0 : 0.32, delay: reduce ? 0 : 0.1 }}
        />
      </svg>
      <div className="absolute inset-0 flex flex-col items-center justify-center pointer-events-none">
        <motion.span
          className="leading-none tabular-nums"
          style={{ font: '800 16px var(--font-sans)', letterSpacing: '-0.02em', color: 'var(--t-title)' }}
          initial={reduce ? false : { opacity: 0, y: 3 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ duration: reduce ? 0 : 0.3, delay: reduce ? 0 : 0.16 }}
        >
          {pct}%
        </motion.span>
        <span style={{ fontSize: 9, color: 'var(--t-faint)' }}>of plan</span>
      </div>
    </div>
  )
}

/** One figure and its label, on one line - the walkthrough's stacked shape. */
function Stat({ value, label, accent = false, delay }: {
  value: string; label: string; accent?: boolean; delay: number
}) {
  const reduce = useReducedMotion()
  return (
    <motion.div
      className="flex items-baseline gap-2 min-w-0"
      initial={reduce ? false : { opacity: 0, y: 5 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: reduce ? 0 : 0.3, delay: reduce ? 0 : delay }}
    >
      <span
        className="tabular-nums"
        style={{
          font: '800 15px var(--font-sans)',
          letterSpacing: '-0.02em',
          color: accent ? 'var(--accent)' : 'var(--t-title)',
        }}
      >
        {value}
      </span>
      <span
        className="truncate"
        style={{ fontSize: 9.5, letterSpacing: '.06em', color: 'var(--t-faint)', textTransform: 'uppercase' }}
      >
        {label}
      </span>
    </motion.div>
  )
}

export function DayScore({ plan, adherence, planned, loggedMinutes, bonusCount, workstreamCount, focusSeconds }: {
  plan: PlanVerdict[]
  adherence: Adherence
  planned: boolean
  loggedMinutes: number
  bonusCount: number
  workstreamCount: number
  focusSeconds: number
}) {
  if (!planned) {
    // No plan, no donut - a day nobody planned did not fail an exercise, and an empty
    // meter is a screen punishing them for not using a feature.
    return (
      <div className="flex flex-col gap-2">
        <Stat value={fmtDur(loggedMinutes * 60)} label="time logged" delay={0.06} />
        <Stat value={`${workstreamCount}`} label="things worked on" delay={0.11} />
        <Stat value={fmtDur(focusSeconds)} label="focused" delay={0.16} />
      </div>
    )
  }

  return (
    <div className="flex items-center gap-5">
      {plan.length > 0 && <PlanDonut pct={adherence.achievement_pct} />}
      <div className="flex flex-col gap-2 min-w-0">
        <Stat value={`${adherence.done} / ${adherence.planned}`} label="planned done" delay={0.1} />
        <Stat value={fmtDur(loggedMinutes * 60)} label="time logged" delay={0.15} />
        {/* Only when there IS extra. A "+0 picked up" every evening reads as a
            target missed rather than a fact absent. */}
        {bonusCount > 0 && (
          <Stat value={`+${bonusCount}`} label="picked up" accent delay={0.2} />
        )}
      </div>
    </div>
  )
}
