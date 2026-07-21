//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The plan, drawn as a ring. The one thing this screen is remembered by.
//
// NOT a percentage donut. A donut draws achievement as a continuous quantity, and
// a plan is not continuous - it is five specific things you named this morning.
// So the ring is SEGMENTED: one arc per planned ticket, evenly divided, with a gap
// between them. You can count your plan in it. A day where you got three of five
// looks like three of five, before you have read a single number.
//
// The outcome is carried by FILL WEIGHT, never by hue:
//
//   done         solid accent
//   partial      accent at low opacity on the same track
//   not touched  nothing - the bare track shows through
//
// One accent, no traffic lights. A red arc for an untouched ticket would be the
// screen telling someone off for a day that was probably fine - the tone contract
// in the prompt forbids that in words, and it has to hold in pixels too.
//
// The ring and the ledger beside it are the same array: hovering a ledger row
// lifts its segment (`activeIndex`), which is what makes the ring readable as a
// list rather than as decoration.
//
// # Who calls this
// `DaySummaryOverlay`, on a day that had a committed plan.
//
// # Related
// - `PlanLedger` - the same verdicts as rows.
// - `DayShape` - the same idea for a day with no plan, rotated flat.

'use client'

import { motion, useReducedMotion } from 'framer-motion'
import type { Adherence, PlanVerdict } from '@/lib/api-types'

const SIZE = 168
const STROKE = 13
const R = (SIZE - STROKE) / 2
const C = 2 * Math.PI * R

/** Visible space between two segments, in px along the ring.
 *
 *  Measured in PIXELS, not degrees, and the round line caps are paid for
 *  explicitly below. A round cap extends an arc by half the stroke width at each
 *  end, so a gap specified in degrees silently shrinks by a whole `STROKE` once
 *  drawn - at six segments that ate the gap entirely and the ring rendered as one
 *  continuous band, which is the exact thing segmenting it is for. */
const GAP_PX = 7

/** How solidly one outcome fills its arc.
 *
 *  An untouched segment draws NOTHING and lets the track below show through -
 *  every segment sits on the same track, so the ring is countable whatever the
 *  outcomes were. Drawing untouched in a hairline instead made the whole ring read
 *  as one pale circle with a couple of arcs on it, and the point of segmenting it
 *  is that a plan of six looks like six. */
function fillOf(outcome: PlanVerdict['outcome']): { color: string; opacity: number } | null {
  switch (outcome) {
    case 'done':
      return { color: 'var(--accent)', opacity: 1 }
    case 'partial':
      return { color: 'var(--accent)', opacity: 0.4 }
    default:
      return null
  }
}

export function AchievementRing({
  plan,
  adherence,
  activeIndex,
  onHover,
}: {
  plan: PlanVerdict[]
  adherence: Adherence
  /** Index of the ledger row under the cursor, or null. */
  activeIndex: number | null
  onHover: (index: number | null) => void
}) {
  const reduce = useReducedMotion()
  const n = Math.max(plan.length, 1)
  const slice = C / n
  // A single planned ticket has no neighbour to be separated from, so it draws as
  // one unbroken ring rather than a circle with a notch cut out of it. Otherwise
  // the gap costs GAP_PX of visible space plus one STROKE for the two round caps
  // that bracket it; a very long plan falls back to whatever room is left rather
  // than inverting into a negative dash.
  const gap = n === 1 ? 0 : Math.min(GAP_PX + STROKE, slice * 0.6)
  const arcLen = Math.max(slice - gap, 1)
  const offsetOf = (i: number) => -(i * slice + gap / 2)

  return (
    <div className="relative shrink-0" style={{ width: SIZE, height: SIZE }}>
      <svg width={SIZE} height={SIZE} viewBox={`0 0 ${SIZE} ${SIZE}`} aria-hidden>
        {/* Rotated so the first planned ticket starts at twelve o'clock and the
            ring reads clockwise, the way the plan itself is ordered. */}
        <g transform={`rotate(-90 ${SIZE / 2} ${SIZE / 2})`}>
          {/* The track pass: one arc per planned ticket, always drawn. This is what
              makes the ring countable - the gaps between these arcs are the plan's
              own divisions, visible whether or not anything got done. It also
              carries the hover target, so an untouched segment still lights its
              ledger row. */}
          {plan.map((v, i) => (
            <circle
              key={`track-${v.task_key}`}
              cx={SIZE / 2}
              cy={SIZE / 2}
              r={R}
              fill="none"
              stroke="color-mix(in srgb, var(--t-title) 11%, var(--t-track))"
              strokeWidth={STROKE}
              strokeLinecap="round"
              strokeDasharray={`${arcLen} ${C - arcLen}`}
              strokeDashoffset={offsetOf(i)}
              opacity={activeIndex !== null && activeIndex !== i ? 0.45 : 1}
              onMouseEnter={() => onHover(i)}
              onMouseLeave={() => onHover(null)}
            />
          ))}

          {/* The fill pass, on top. Untouched segments contribute nothing here. */}
          {plan.map((v, i) => {
            const fill = fillOf(v.outcome)
            if (!fill) return null
            const dim = activeIndex !== null && activeIndex !== i
            return (
              <motion.circle
                key={v.task_key}
                cx={SIZE / 2}
                cy={SIZE / 2}
                r={R}
                fill="none"
                stroke={fill.color}
                strokeWidth={STROKE}
                strokeLinecap="round"
                strokeDasharray={`${arcLen} ${C - arcLen}`}
                strokeDashoffset={offsetOf(i)}
                initial={reduce ? false : { opacity: 0 }}
                animate={{
                  // Dimming the rest on hover rather than brightening the one is
                  // what keeps a hovered segment from looking like a new state.
                  opacity: dim ? fill.opacity * 0.3 : fill.opacity,
                }}
                transition={{ duration: reduce ? 0 : 0.32, delay: reduce ? 0 : 0.06 * i }}
                style={{ pointerEvents: 'none' }}
              />
            )
          })}
        </g>
      </svg>

      {/* The well. The percentage is the smaller claim here, not the bigger one -
          "4 of 5" is what a person actually recognises about their own day, and
          the percentage is the summary of it. */}
      <div className="absolute inset-0 flex flex-col items-center justify-center pointer-events-none">
        <motion.p
          className="leading-none"
          style={{
            font: '800 34px var(--font-sans)',
            letterSpacing: '-0.035em',
            color: 'var(--t-title)',
          }}
          initial={reduce ? false : { opacity: 0, y: 4 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ duration: reduce ? 0 : 0.3, delay: reduce ? 0 : 0.18 }}
        >
          {adherence.done}
          <span style={{ color: 'var(--t-faint-2)', fontWeight: 600 }}> / {adherence.planned}</span>
        </motion.p>
        <motion.p
          className="mt-label mt-2"
          style={{ color: 'var(--t-faint-2)' }}
          initial={reduce ? false : { opacity: 0 }}
          animate={{ opacity: 1 }}
          transition={{ duration: reduce ? 0 : 0.3, delay: reduce ? 0 : 0.26 }}
        >
          {adherence.achievement_pct}% of the plan
        </motion.p>
      </div>
    </div>
  )
}
