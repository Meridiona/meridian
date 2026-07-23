//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The wait, while the day is being written up.
//
// This call takes tens of seconds - it reads the whole day and then makes one LLM
// call through whatever provider the user configured. A single static sentence for
// that long reads as "hung" somewhere around second fifteen, and the button gets
// clicked again. So the wait says what is happening, and keeps moving.
//
// THE RING IS THE POINT. It is the same geometry as AchievementRing - same size,
// same stroke, same track - with one arc sweeping around it. What you are watching
// is the shape that is about to fill in with your own day, which makes the wait feel
// like the screen arriving rather than a spinner standing in for it.
//
// HONEST, NOT FAKE. There is no percentage and no creeping bar: we genuinely do not
// know how far along the model is, and a progress figure the user can time is a lie.
// What DOES advance is the caption, and only through stages that really do happen in
// that order (read the day, gather the work, weigh it against the plan, write). The
// timings are a good guess at their pace, not a measurement - which is why the last
// line stays up until the answer lands rather than completing and leaving the ring
// spinning under a finished sentence.
//
// # Related
// - `GeneratingBar` - the same idea for a narrow inline panel (worklog drafts, the
//   task composer). This one exists because that one is a 4px bar built for a
//   388px column and cannot carry a whole empty screen.

'use client'

import { useEffect, useState } from 'react'
import { AnimatePresence, motion, useReducedMotion } from 'framer-motion'

const SIZE = 108
const STROKE = 9
const R = (SIZE - STROKE) / 2
const C = 2 * Math.PI * R

/** What is actually happening, in the order it happens.
 *
 *  `at` is seconds from the start. Spaced against a real run (~26s end to end on a
 *  hosted model): early stages are quick DB reads, and the last one owns most of
 *  the wall clock because it is the model writing. */
function steps(planned: boolean): { at: number; text: string }[] {
  return [
    { at: 0, text: 'Reading your day' },
    { at: 4, text: 'Gathering everything you worked on' },
    planned
      ? { at: 9, text: 'Weighing it against what you planned' }
      : { at: 9, text: 'Working out what the day was about' },
    { at: 15, text: 'Writing it up' },
  ]
}

export function Composing({ planned }: { planned: boolean }) {
  const reduce = useReducedMotion()
  const [elapsed, setElapsed] = useState(0)

  useEffect(() => {
    const t = setInterval(() => setElapsed(e => e + 1), 1000)
    return () => clearInterval(t)
  }, [])

  const stages = steps(planned)
  // The LAST stage whose time has passed - so a slow run holds on "Writing it up"
  // rather than running out of things to say.
  const current = stages.filter(s => s.at <= elapsed).at(-1) ?? stages[0]

  return (
    <div className="flex-1 min-h-0 flex flex-col items-center justify-center gap-7" role="status" aria-live="polite">
      <div style={{ width: SIZE, height: SIZE }}>
        <svg width={SIZE} height={SIZE} viewBox={`0 0 ${SIZE} ${SIZE}`} aria-hidden>
          <circle
            cx={SIZE / 2}
            cy={SIZE / 2}
            r={R}
            fill="none"
            stroke="color-mix(in srgb, var(--t-title) 11%, var(--t-track))"
            strokeWidth={STROKE}
          />
          {/* One arc, sweeping. `spin` is the app's own keyframe (globals.css); the
              duration is set here because the shared `.mer-spin` runs at 0.8s,
              which on a ring this size reads as frantic rather than working. */}
          <circle
            cx={SIZE / 2}
            cy={SIZE / 2}
            r={R}
            fill="none"
            stroke="var(--accent)"
            strokeWidth={STROKE}
            strokeLinecap="round"
            strokeDasharray={`${C * 0.22} ${C * 0.78}`}
            style={{
              transformOrigin: 'center',
              // Slowed rather than stopped under reduced motion: this is a status
              // indicator, and a frozen one says the opposite of what it means.
              animation: `spin ${reduce ? 4.5 : 1.9}s linear infinite`,
            }}
          />
        </svg>
      </div>

      <div className="flex flex-col items-center gap-2.5" style={{ minHeight: 52 }}>
        {/* Keyed on the text so each stage crossfades in place. The container holds
            its height so the note below never hops when a longer line lands. */}
        <AnimatePresence mode="wait">
          <motion.p
            key={current.text}
            initial={reduce ? false : { opacity: 0, y: 5 }}
            animate={{ opacity: 1, y: 0 }}
            exit={reduce ? { opacity: 0 } : { opacity: 0, y: -5 }}
            transition={{ duration: reduce ? 0.1 : 0.28 }}
            className="text-center"
            style={{
              font: '600 15px var(--font-sans)',
              letterSpacing: '-0.01em',
              color: 'var(--t-title)',
            }}
          >
            {current.text}
          </motion.p>
        </AnimatePresence>

        <p className="mt-body-sm text-center" style={{ color: 'var(--t-faint-2)', maxWidth: '48ch' }}>
          Usually under a minute - you can leave this open.
        </p>
      </div>
    </div>
  )
}
