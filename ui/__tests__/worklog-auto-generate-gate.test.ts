//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'fs'

// Worklog auto-generate must not be gated on a connected tracker.
//
// The same wrong premise was written in THREE places - the backend sweep
// (`src/pm_worklog/auto_generate.rs`), the Settings tab, and the one-time Timeline
// nudge - each on the reasoning that "there is nothing to draft against without a
// tracker". That is false: `pm_worklog::generate` matches a PERSONAL (`local`)
// day-task exactly like a real ticket, and on approve posts to that task's own row
// (`post_to_local_task`). Only the PROPOSE branch needs a configured provider.
//
// The combined effect on a solo user - the cohort Meridian now leads with - was that
// the nudge never fired, so they were never offered the feature, AND Settings →
// Worklogs rendered as a single dead-end sentence with no toggle and nothing to
// click, so there was no second way to find it. The feature was effectively absent
// from the product while appearing to be shipped.
//
// Source-level assertions, as elsewhere in this suite: there is no React render
// harness here, and the failure mode is a missing render rather than a wrong value.

const uiRoot = import.meta.dir + '/..'
const section = readFileSync(`${uiRoot}/components/timeline/settings/WorklogSection.tsx`, 'utf8')
const shell = readFileSync(`${uiRoot}/components/timeline/MeridianTimelineShell.tsx`, 'utf8')

/** Strip comment lines - both files explain the removed gate in prose. */
const code = (src: string) =>
  src.split('\n').filter((l) => !l.trim().startsWith('//') && !l.trim().startsWith('*')).join('\n')

describe('worklog auto-generate is offered without a tracker', () => {
  it('the Settings tab renders its controls unconditionally', () => {
    const body = code(section)
    // The dead end: the whole card behind a `!hasTracker ? … : …`.
    expect(/!hasTracker\s*\?/.test(body)).toBe(false)
    // The controls that must always be reachable.
    expect(body).toContain('<Switch')
    expect(body).toContain('<SaveButton')
    expect(body).toContain('Auto-draft worklogs')
  })

  it('the tracker line is an offer at the end, not a wall at the start', () => {
    const body = code(section)
    const note = body.indexOf('!hasTracker &&')
    expect(note).toBeGreaterThan(-1)
    // After the card it used to replace, not before it.
    expect(note).toBeGreaterThan(body.indexOf('</SectionCard>'))
  })

  it('the one-time nudge is not gated on a tracker', () => {
    const body = code(shell)
    // `worklogPrompted !== false` is the real condition - has the user answered yet.
    expect(body).toContain('if (worklogPrompted !== false) return')
    expect(/worklogPrompted !== false \|\| !hasTracker/.test(body)).toBe(false)
  })
})
