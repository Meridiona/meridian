//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The live-hour card's copy contract.
//
// The failure this closes is a silent one: `HourTakeover` has always accepted a
// `nextHourLabel` prop and substituted a `{NEXT}` placeholder, the caller has
// always passed a real clock label - and not one line of queued copy contained
// the placeholder. The wiring worked perfectly and did nothing, so the card told
// the user their hour would be written up "at the top of the hour" while it knew
// the actual time and had a slot to print it in. Nothing fails when that
// regresses; the copy just quietly gets vaguer.

import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'

const src = readFileSync(join(import.meta.dir, '..', 'components/timeline/HourBadges.tsx'), 'utf8')
const copy = src.slice(src.indexOf('const COPY'), src.indexOf('const SOLO_GENERATING_TITLE'))
const queued = copy.slice(copy.indexOf('queued: ['))
/** The second string of each `[headline, sub]` pair. */
const subs = (s: string) => s.match(/, '([^']*)'\]/g) ?? []

describe('the live-hour card', () => {
  it('names the clock time on every queued line, rather than "the hour"', () => {
    // Three rotating variants; the user sees exactly one, so a line without the
    // placeholder is a whole cohort of users told nothing useful.
    expect(subs(queued).length).toBe(3)
    for (const s of subs(queued)) expect(s).toContain('{NEXT}')
  })

  it('substitutes it from the prop, not from a clock of its own', () => {
    // A second time source here could disagree with the rail the card sits
    // under - see `hourClock(nowHour + 1)` in DayTaskColumn.
    expect(src).toContain("subTemplate.replace('{NEXT}', nextHourLabel)")
    expect(src).not.toMatch(/new Date\(/)
  })

  it('ends on the notification, so the user knows not to come back and look', () => {
    for (const s of subs(queued)) expect(s).toContain('notify you')
  })

  it('keeps the displayed strings free of em-dashes', () => {
    // Repo hard rule. Scoped to COPY, since prose comments are exempt from it.
    expect(copy).not.toMatch(/[—–]/)
  })
})
