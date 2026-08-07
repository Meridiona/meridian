//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// A worklog draft that has fallen behind the work it describes.
//
// The failure this closes: a draft is written from the work that existed when it
// was generated, and the user keeps working - which is the NORMAL case, since
// drafts are generated mid-afternoon and the afternoon carries on. Nothing said
// so. Either an update went out missing half a day, or the user noticed once and
// stopped trusting the drafts; both are silent and both cost the feature.
//
// The staleness figure itself is computed and tested in Rust (measured minutes
// now, minus the same figure captured at draft time - see
// `meridian-core/src/readers/day_task_worklogs/`). These pin the UI contract:
// that this side never re-derives the threshold, and that the states a user can
// be in are all rendered.

import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { staleAmount } from '../components/timeline/WorklogDraftDialog'

const src = (p: string) => readFileSync(join(import.meta.dir, '..', p), 'utf8')
const dialog = src('components/timeline/WorklogDraftDialog.tsx')
const types = src('lib/api-types.ts')

describe('staleAmount', () => {
  it('reads as minutes below the hour', () => {
    expect(staleAmount(1)).toBe('1 minutes')
    expect(staleAmount(18)).toBe('18 minutes')
    expect(staleAmount(59)).toBe('59 minutes')
  })

  it('switches to hours rather than saying "90 minutes"', () => {
    expect(staleAmount(60)).toBe('1 hour')
    expect(staleAmount(90)).toBe('1 hour 30 min')
    expect(staleAmount(120)).toBe('2 hours')
    expect(staleAmount(125)).toBe('2 hours 5 min')
  })

  it('survives the two values the backend can legitimately send', () => {
    // null = no baseline (drafted before migration 077, or the task is gone).
    // The banner is gated on `stale`, which is false in both cases, so this only
    // has to not throw.
    expect(staleAmount(null)).toBe('0 minutes')
    // The Rust side clamps, but a negative here must not render "-8 minutes".
    expect(staleAmount(-8)).toBe('0 minutes')
  })
})

describe('the UI never re-derives what "stale" means', () => {
  it('reads the boolean the daemon computed, not a threshold of its own', () => {
    // A second definition in TypeScript is a second definition that can
    // disagree with the notification the user has already been sent - so the
    // toast says "rewrite this" and the screen says "ready for review".
    expect(dialog).toContain('draft?.stale')
    expect(dialog).toContain('draft.stale &&')
    // No local threshold anywhere: the only comparison is against zero, in the
    // formatter, and that is a clamp rather than a rule.
    expect(dialog).not.toMatch(/stale_minutes\s*[><]=?\s*\d/)
  })

  it('documents both fields as coming from the measured spans', () => {
    expect(types).toContain('stale_minutes: number | null')
    expect(types).toContain('stale: boolean')
    expect(types).toContain('WORKLOG_STALE_MINUTES')
  })
})

describe('every state the user can be in is rendered', () => {
  it('says "out of date" before it says "ready for review"', () => {
    // Order is the whole point: "ready for review" over an update missing the
    // last hour is confidently wrong, and the user finds out after it is live.
    expect(dialog.indexOf('draft?.stale')).toBeLessThan(dialog.indexOf("tone=\"ready\""))
  })

  it('names the amount rather than saying only "out of date"', () => {
    // Which of several drafts is worth the click, and whether to do it now or
    // at the end of the day, are both decided by the number.
    expect(dialog).toContain('${staleAmount(draft.stale_minutes)} of work missing')
    expect(dialog).toContain('{staleAmount(minutes)} of work is missing from this')
  })

  it('puts the fix inside the warning', () => {
    // A warning that leaves the user to find the control is a warning that gets
    // dismissed. Same `generate` the action row's Rewrite calls - one path.
    expect(dialog).toContain('onRegenerate={generate}')
    expect(dialog).toContain('function StaleBanner')
    const banner = dialog.slice(dialog.indexOf('function StaleBanner'))
    expect(banner).toContain('Rewrite')
  })

  it('warns above the document, not below it', () => {
    // Everything under it is text the user is about to judge; put at the bottom
    // it would be found after the decision it should have informed.
    const body = dialog.slice(dialog.indexOf('Body — the document surface'))
    expect(body.indexOf('<StaleBanner')).toBeLessThan(body.indexOf('<DraftDocument'))
  })
})
