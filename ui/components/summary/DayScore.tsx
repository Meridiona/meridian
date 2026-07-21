//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The day's numbers, with the plan drawn as a ring you can count.
//
// WHY A SEGMENTED RING. A plain progress bar reads as one continuous quantity, and
// a plan is not continuous - it is five specific things named this morning. So the
// ring is divided into one arc per planned ticket, with a gap between them: three of
// five looks like three of five before a number is read. An earlier take used a
// flat segmented bar under the figures and it read as a loading indicator, not an
// achievement - the ring is the thing this screen is remembered by.
//
// The outcome is carried by FILL WEIGHT, never by hue:
//
//   done         solid accent
//   partial      accent at low opacity, on the same track
//   not touched  the bare track
//
// One accent, no traffic lights. A red arc for an untouched ticket would be the
// screen telling someone off for a day that was probably fine.
//
// EVERY NUMBER HERE IS MEASURED. The counts and the percentage come from
// `day_evidence::adherence` in Rust; the durations are summed from the same
// day-task list the rows below render. The model contributes no figure.
//
// # Related
// - `WorkList` - the same plan and the same work, itemised.
// - `DayShape` - what stands in for the ring on a day with no plan.

'use client'

import { motion, useReducedMotion } from 'framer-motion'
import { fmtDur } from '@/components/atoms'
import type { Adherence, PlanVerdict } from '@/lib/api-types'

const SIZE = 132
const STROKE = 12
const R = (SIZE - STROKE) / 2
const C = 2 * Math.PI * R
// Gap between two arcs, in px along the ring. Measured in pixels, not degrees,
// because the round caps each add STROKE/2 per end - a degree gap silently closes
// up at five or six segments, which is the exact thing dividing the ring is for.
const GAP_PX = 6

function fillOpacity(outcome: PlanVerdict['outcome']): number | null {
  switch (outcome) {
    case 'done': return 1
    case 'partial': return 0.38
    default: return null
  }
}

/** The segmented plan ring. */
function PlanRing({ plan, adherence }: { plan: PlanVerdict[]; adherence: Adherence }) {
  const reduce = useReducedMotion()
  const n = Math.max(plan.length, 1)
  const slice = C / n
  const gap = n === 1 ? 0 : Math.min(GAP_PX + STROKE, slice * 0.55)
  const arcLen = Math.max(slice - gap, 1)
  const offsetOf = (i: number) => -(i * slice + gap / 2)
  const track = 'color-mix(in srgb, var(--t-title) 11%, var(--t-track))'

  return (
    <div className="relative shrink-0" style={{ width: SIZE, height: SIZE }}>
      <svg width={SIZE} height={SIZE} viewBox={`0 0 ${SIZE} ${SIZE}`} aria-hidden>
        <g transform={`rotate(-90 ${SIZE / 2} ${SIZE / 2})`}>
          {plan.map((v, i) => (
            <circle
              key={`t-${v.task_key}`}
              cx={SIZE / 2} cy={SIZE / 2} r={R} fill="none"
              stroke={track} strokeWidth={STROKE} strokeLinecap="round"
              strokeDasharray={`${arcLen} ${C - arcLen}`} strokeDashoffset={offsetOf(i)}
            />
          ))}
          {plan.map((v, i) => {
            const op = fillOpacity(v.outcome)
            if (op === null) return null
            return (
              <motion.circle
                key={v.task_key}
                cx={SIZE / 2} cy={SIZE / 2} r={R} fill="none"
                stroke="var(--accent)" strokeWidth={STROKE} strokeLinecap="round"
                strokeDasharray={`${arcLen} ${C - arcLen}`} strokeDashoffset={offsetOf(i)}
                initial={reduce ? false : { opacity: 0 }}
                animate={{ opacity: op }}
                transition={{ duration: reduce ? 0 : 0.32, delay: reduce ? 0 : 0.1 + 0.06 * i }}
              />
            )
          })}
        </g>
      </svg>
      <div className="absolute inset-0 flex flex-col items-center justify-center pointer-events-none">
        <motion.p
          className="leading-none tabular-nums"
          style={{ font: '800 30px var(--font-sans)', letterSpacing: '-0.035em', color: 'var(--t-title)' }}
          initial={reduce ? false : { opacity: 0, y: 3 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ duration: reduce ? 0 : 0.3, delay: reduce ? 0 : 0.16 }}
        >
          {adherence.achievement_pct}%
        </motion.p>
        <p className="mt-label mt-1.5" style={{ color: 'var(--t-faint-2)' }}>of plan</p>
      </div>
    </div>
  )
}

/** One figure and its label. */
function Stat({ value, label, accent = false, delay }: {
  value: string; label: string; accent?: boolean; delay: number
}) {
  const reduce = useReducedMotion()
  return (
    <motion.div
      className="flex flex-col gap-1.5 min-w-0"
      initial={reduce ? false : { opacity: 0, y: 5 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: reduce ? 0 : 0.3, delay: reduce ? 0 : delay }}
    >
      <p
        className="leading-none tabular-nums truncate"
        style={{
          font: '800 25px var(--font-sans)',
          letterSpacing: '-0.035em',
          color: accent ? 'var(--accent)' : 'var(--t-title)',
        }}
      >
        {value}
      </p>
      <p className="mt-label truncate" style={{ color: 'var(--t-faint-2)' }}>{label}</p>
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
    // No plan, no ring - a day nobody planned did not fail an exercise, and an empty
    // meter is a screen punishing them for not using a feature.
    return (
      <div className="flex items-start gap-8 sm:gap-11 flex-wrap">
        <Stat value={fmtDur(loggedMinutes * 60)} label="time logged" delay={0.06} />
        <Stat value={`${workstreamCount}`} label="things worked on" delay={0.11} />
        <Stat value={fmtDur(focusSeconds)} label="focused" delay={0.16} />
      </div>
    )
  }

  return (
    <div className="flex items-center gap-7 sm:gap-9 flex-wrap">
      {plan.length > 0 && <PlanRing plan={plan} adherence={adherence} />}
      <div className="flex items-start gap-7 sm:gap-9 flex-wrap">
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
