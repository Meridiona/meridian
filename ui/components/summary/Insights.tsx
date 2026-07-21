//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Three cards, three angles on the day the numbers cannot carry.
//
// Each card has a JOB, left to right, and the prompt is written to fill them in
// order:
//
//   1  how the day went overall - ahead, steady, scattered, loosely planned
//   2  the standout win - one genuinely good thing, and only a good thing
//   3  a nice find - something new, a pattern, an unusual stretch
//
// But the job is the only fixed thing. The heading and the line under it are always
// the model's own words: an earlier shape had a closed vocabulary (`achieved` /
// `drifted`) and it turned into slots that got filled whether or not the day had
// anything to say. A card names what it actually found; card 1 on a scattered day
// says so, on a great one it does not.
//
// The colour and the glyph come from POSITION, not from any classification the
// model made - so a card is never red because the news is bad, and the model is
// never asked to grade its own remark. Fixed hex (not theme tokens): these carry
// white text and are the screen's one bold moment, so they must look identical in
// light and dark rather than washing out with the surface.
//
// # Related
// - `DayScore` - the measured half of the same story, directly above.

'use client'

import { motion, useReducedMotion } from 'framer-motion'
import type { DaySummaryInsight } from '@/lib/api-types'

/** Per-position styling: fill + the glyph drawn on it. In order — the day overall,
 *  the win, the find. */
const CARDS = [
  {
    fill: 'linear-gradient(158deg, #6E4BFF 0%, #5A38E8 100%)',
    // A gauge - "how the day went".
    glyph: (
      <path
        d="M2.5 11a6.5 6.5 0 1 1 13 0M9 11l3.2-3.2"
        stroke="#fff" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" fill="none"
      />
    ),
  },
  {
    fill: 'linear-gradient(158deg, #E4693C 0%, #D2502C 100%)',
    // A trophy - the standout win.
    glyph: (
      <path
        d="M5 3h8v2.5a4 4 0 0 1-8 0V3ZM5 4H3.2a2 2 0 0 0 2 3M13 4h1.8a2 2 0 0 1-2 3M7.5 9.2 7 12h4l-.5-2.8M6 14h6"
        stroke="#fff" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" fill="none"
      />
    ),
  },
  {
    fill: 'linear-gradient(158deg, #14A08C 0%, #0C8674 100%)',
    // A four-point spark - the nice find.
    glyph: <path d="M9 2.5l1.7 4.8L15.5 9l-4.8 1.7L9 15.5l-1.7-4.8L2.5 9l4.8-1.7z" fill="#fff" />,
  },
]

function Glyph({ index }: { index: number }) {
  return (
    <span
      className="shrink-0 flex items-center justify-center rounded-lg"
      style={{ width: 26, height: 26, background: 'rgba(255,255,255,0.16)' }}
    >
      <svg width="18" height="18" viewBox="0 0 18 18" aria-hidden>
        {CARDS[index % CARDS.length].glyph}
      </svg>
    </span>
  )
}

export function Insights({ insights, delay = 0 }: { insights: DaySummaryInsight[]; delay?: number }) {
  const reduce = useReducedMotion()
  const cards = insights.filter(i => i.text.trim()).slice(0, 3)
  if (cards.length === 0) return null

  return (
    <div
      className="grid gap-3"
      // Sized to how many cards there actually are: two cards stretched across
      // three columns leaves a hole that reads as one that failed to load.
      style={{ gridTemplateColumns: `repeat(${cards.length}, minmax(0, 1fr))` }}
    >
      {cards.map((insight, i) => (
        <motion.div
          key={i}
          className="rounded-xl px-4 py-3.5 flex flex-col gap-2.5"
          style={{ background: CARDS[i % CARDS.length].fill }}
          initial={reduce ? false : { opacity: 0, y: 8 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ duration: reduce ? 0 : 0.35, delay: reduce ? 0 : delay + 0.06 * i }}
        >
          <div className="flex items-center gap-2.5">
            <Glyph index={i} />
            {insight.title && (
              <p style={{
                font: '700 13.5px var(--font-sans)',
                letterSpacing: '-0.01em',
                color: '#fff',
              }}>
                {insight.title}
              </p>
            )}
          </div>
          <p style={{
            font: '450 12.5px/1.5 var(--font-sans)',
            letterSpacing: '-0.004em',
            color: 'rgba(255,255,255,0.88)',
          }}>
            {insight.text}
          </p>
        </motion.div>
      ))}
    </div>
  )
}
