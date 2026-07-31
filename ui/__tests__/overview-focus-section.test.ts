//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// OverviewPanel's "Today's focus" visibility rule.
//
// The bug: the populated checklist and a past day's note were gated on
// `!isSolo` while the empty-today nudge was shown to everyone. A solo user was
// therefore invited to plan, allowed to commit a personal task, and then shown
// nothing — the section vanished the instant the plan stopped being empty.
// Reproduced on a real install: daily_plan had one confirmed row for the day
// (daily_plan_meta.confirmed_at set, skipped=0) and the panel rendered no
// focus section at all.
//
// Two halves are pinned here, because either alone would pass while the other
// regressed: the predicate's truth table, and the fact that the JSX actually
// calls it rather than re-deriving a gate inline (this repo has no React
// render harness — see task-composer.test.ts).

import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { focusSectionVisible } from '../components/timeline/types'

const src = (p: string) => readFileSync(join(import.meta.dir, '..', p), 'utf8')
const panel = src('components/timeline/OverviewPanel.tsx')

/** The pre-fix gate, kept verbatim as the reference for the parity checks. */
function legacyGate(a: {
  isToday: boolean
  planLoaded: boolean
  itemCount: number
  isSolo: boolean
}): boolean {
  return (
    (a.isToday && a.planLoaded && a.itemCount === 0) ||
    (!a.isSolo && (a.itemCount > 0 || !a.isToday))
  )
}

const BOOLS = [true, false]
const COUNTS = [0, 1, 3]

describe('today, with a committed plan', () => {
  it('renders the checklist for a solo user — the reported bug', () => {
    expect(focusSectionVisible({ isToday: true, planLoaded: true, itemCount: 1 })).toBe(true)
  })

  it('rendered nothing under the old gate, which is what made it a bug', () => {
    expect(legacyGate({ isToday: true, planLoaded: true, itemCount: 1, isSolo: true })).toBe(false)
  })

  it('renders for a connected user too, unchanged', () => {
    expect(focusSectionVisible({ isToday: true, planLoaded: true, itemCount: 1 })).toBe(true)
    expect(legacyGate({ isToday: true, planLoaded: true, itemCount: 1, isSolo: false })).toBe(true)
  })
})

describe('today, with nothing planned', () => {
  it('renders the nudge once get_plan has resolved', () => {
    expect(focusSectionVisible({ isToday: true, planLoaded: true, itemCount: 0 })).toBe(true)
  })

  it('renders nothing before the first fetch, so the nudge cannot flash', () => {
    expect(focusSectionVisible({ isToday: true, planLoaded: false, itemCount: 0 })).toBe(false)
  })
})

describe('a past date', () => {
  it('always renders — committed focus, or a quiet empty note', () => {
    for (const planLoaded of BOOLS) {
      for (const itemCount of COUNTS) {
        expect(focusSectionVisible({ isToday: false, planLoaded, itemCount })).toBe(true)
      }
    }
  })
})

describe('the full truth table', () => {
  // isToday × planLoaded × itemCount, exhaustively. Written out rather than
  // derived so a change to the rule has to be stated here deliberately.
  const cases: Array<[boolean, boolean, number, boolean]> = [
    // isToday, planLoaded, itemCount, expected
    [true, true, 0, true], //  nudge
    [true, false, 0, false], // pre-fetch: nothing, no flash
    [true, true, 1, true], //  checklist
    [true, false, 1, true], //  items imply a resolved fetch
    [false, true, 0, true], //  past, empty note
    [false, false, 0, true], //  past, empty note
    [false, true, 1, true], //  past, read-only focus
    [false, false, 1, true], //  past, read-only focus
  ]

  for (const [isToday, planLoaded, itemCount, expected] of cases) {
    it(`isToday=${isToday} planLoaded=${planLoaded} items=${itemCount} → ${expected}`, () => {
      expect(focusSectionVisible({ isToday, planLoaded, itemCount })).toBe(expected)
    })
  }
})

describe('connected users are unaffected', () => {
  it('matches the old gate exactly for every non-solo combination', () => {
    for (const isToday of BOOLS) {
      for (const planLoaded of BOOLS) {
        for (const itemCount of COUNTS) {
          expect(focusSectionVisible({ isToday, planLoaded, itemCount })).toBe(
            legacyGate({ isToday, planLoaded, itemCount, isSolo: false }),
          )
        }
      }
    }
  })

  it('now treats solo and connected identically, which it did not before', () => {
    const differed = [] as string[]
    for (const isToday of BOOLS) {
      for (const planLoaded of BOOLS) {
        for (const itemCount of COUNTS) {
          const solo = legacyGate({ isToday, planLoaded, itemCount, isSolo: true })
          const connected = legacyGate({ isToday, planLoaded, itemCount, isSolo: false })
          if (solo !== connected) differed.push(`${isToday}/${planLoaded}/${itemCount}`)
        }
      }
    }
    // The old gate genuinely diverged — if it didn't, this test is guarding nothing.
    expect(differed.length).toBeGreaterThan(0)
  })
})

describe('the panel uses the shared predicate', () => {
  it('calls focusSectionVisible for the focus section', () => {
    expect(panel).toContain('focusSectionVisible({')
    expect(panel).toContain("import { focusSectionVisible")
  })

  it('does not gate the focus section on isSolo any more', () => {
    const gate = panel.split('\n').find(l => l.includes('focusSectionVisible({'))
    expect(gate).toBeDefined()
    expect(gate).not.toContain('isSolo')
    // The exact pre-fix expression must not come back.
    expect(panel).not.toContain('!isSolo && (focusItems.length > 0')
  })

  it('still uses isSolo elsewhere, so the fix did not over-reach', () => {
    // Solo genuinely changes other things (greeting copy, the Drafts mini-card,
    // the connect-a-tracker CTA) — those are not part of this bug.
    expect(panel).toContain('isSolo')
  })
})
