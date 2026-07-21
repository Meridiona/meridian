//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The observations. Unlabelled, on purpose.
//
// An earlier shape gave every insight a category - `achieved`, `overperformed`,
// `drifted` - and rendered each as a titled card. It read as a performance review.
// A line that says "the triage work was not planned but it unblocked two tickets"
// is an observation; the same line under a heading reading DRIFTED is a verdict on
// the person, and no amount of gentle wording underneath undoes the heading.
//
// So: plain rows, one hairline mark each, no titles.
//
// The ONE exception is `learned`. Something the day genuinely taught is not an
// observation about the day, it is a thing you now know - and it is the single
// most feel-good item this screen can carry, so it gets lifted out into its own
// card. **When there are none the card does not exist**: an empty "what you
// learned" frame every evening would push the model toward manufacturing one, and
// a manufactured lesson is a lie the reader can feel.

'use client'

import { motion, useReducedMotion } from 'framer-motion'
import type { DaySummaryInsight } from '@/lib/api-types'

/** The four-point star used for the learned card. Drawn rather than an emoji: an
 *  emoji renders at the OS's whim and in its own palette. */
function Spark() {
  return (
    <svg width="13" height="13" viewBox="0 0 13 13" fill="none" aria-hidden>
      <path
        d="M6.5 0l1.5 4.6L12.5 6.5 8 8.4 6.5 13 5 8.4.5 6.5 5 4.6z"
        fill="var(--accent)"
      />
    </svg>
  )
}

export function Insights({ insights, delay = 0 }: { insights: DaySummaryInsight[]; delay?: number }) {
  const reduce = useReducedMotion()
  const plain = insights.filter(i => !i.learned)
  const learned = insights.filter(i => i.learned)
  if (insights.length === 0) return null

  return (
    <div className="flex flex-col gap-4">
      {plain.length > 0 && (
        <ul className="flex flex-col gap-2.5">
          {plain.map((i, n) => (
            <motion.li
              key={n}
              className="flex gap-3 items-baseline"
              initial={reduce ? false : { opacity: 0, y: 4 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ duration: reduce ? 0 : 0.3, delay: reduce ? 0 : delay + 0.05 * n }}
            >
              {/* A short rule, not a bullet. A bullet makes a list; a rule makes a
                  set of separate remarks, which is what these are. */}
              <span
                className="shrink-0 self-center"
                style={{ width: 10, height: 1, background: 'var(--t-hair)' }}
              />
              <span className="mt-body" style={{ color: 'var(--t-muted)' }}>
                {i.text}
              </span>
            </motion.li>
          ))}
        </ul>
      )}

      {learned.length > 0 && (
        <motion.div
          className="rounded-xl px-4 py-3"
          style={{
            background: 'color-mix(in srgb, var(--accent) 6%, var(--t-card))',
            border: '1px solid color-mix(in srgb, var(--accent) 18%, transparent)',
          }}
          initial={reduce ? false : { opacity: 0, y: 6 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ duration: reduce ? 0 : 0.35, delay: reduce ? 0 : delay + 0.05 * plain.length }}
        >
          <p className="mt-label flex items-center gap-2 mb-2" style={{ color: 'var(--accent)' }}>
            <Spark />
            Picked up along the way
          </p>
          <ul className="flex flex-col gap-1.5">
            {learned.map((i, n) => (
              <li key={n} className="mt-body" style={{ color: 'var(--t-title)' }}>
                {i.text}
              </li>
            ))}
          </ul>
        </motion.div>
      )}
    </div>
  )
}
