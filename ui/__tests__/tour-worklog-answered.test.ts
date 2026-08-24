//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// RUNTIME harness for `waitForWorklogAnswered` (components/tutorial/scriptDay.ts) —
// the tour beat that decides the worklog-time question has been answered.
//
// Why this is a real test and not a source scan. The beat has now failed twice,
// in two different ways, and neither was visible in the shape of the code:
//
//   1. `waitForClick` resolved `true` with no observable click of any kind, so
//      the tour moved on and the time was never saved (PR #849's investigation,
//      fixed in #852 by polling the setting instead).
//   2. The poll then mapped "the read FAILED" and "settings say X" onto the same
//      `null`, so one rejected `get_settings` made the very next successful poll
//      compare as a change — ending the beat with nothing recorded, i.e. the
//      exact failure #852 exists to prevent, reached through the error path
//      (fixed in #854).
//
// The guard shipped for (2) asserted on source fragments, which cannot tell the
// fix apart from a rewrite that reintroduces the bug with different spacing.
// This drives the real function instead, over the interleaving that broke it.
//
// The settings read is injected rather than module-mocked ON PURPOSE: bun's
// `mock.module` replaces a module for the whole test PROCESS, so stubbing
// `@/lib/bridge` here broke every other test file that imports `invoke` or
// `openExternal` from it (11 failures across 6 files, measured). A parameter
// with a production default touches nothing outside this beat.

import { describe, it, expect, beforeAll, afterAll } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'
import { waitForWorklogAnswered, type SettingsProbe } from '../components/tutorial/scriptDay'

/** The dialog's button. Present = the dialog is up; removing it is itself an
 *  answer ("Not now" / the backdrop), which the function short-circuits on. */
const BUTTON = '<button data-tour="worklog-schedule-on">Turn on</button>'

/** A probe that replays `readings` one per call, repeating the last forever.
 *  `null` means THE READ FAILED (the production probe answers `undefined`).
 *  Also counts calls, so a test can prove the poll actually ran. */
function probeOf(readings: Array<string | null>) {
  const queue = [...readings]
  const state = { calls: 0 }
  const probe: SettingsProbe = async () => {
    state.calls += 1
    const next = queue.length > 1 ? queue.shift() : queue[0]
    return next === null ? undefined : next
  }
  return { probe, state }
}

/** A `Stage` with only the two members this function touches.
 *
 *  `waitForClick` is called for its fence side effect and its RESULT is
 *  deliberately ignored by the code under test (#852) — so a never-resolving
 *  promise is the honest stub: if the function ever starts trusting it again,
 *  these tests hang instead of passing on a value the real primitive cannot be
 *  trusted to produce. `pause` is shortened so a bounded poll runs quickly. */
function stubStage() {
  return {
    waitForClick: () => new Promise<boolean>(() => {}),
    pause: () => new Promise<void>((r) => setTimeout(r, 1)),
  } as never
}

beforeAll(() => GlobalRegistrator.register())
afterAll(() => GlobalRegistrator.unregister())

describe('waitForWorklogAnswered', () => {
  it('does not treat a recovered read as the user answering', async () => {
    // THE REGRESSION. The baseline read fails; every later read succeeds and
    // reports the SAME untouched settings. Nothing was answered, so the beat
    // must keep waiting and time out — not resolve on the first reading that
    // happens to arrive after the failure.
    document.body.innerHTML = BUTTON
    const { probe, state } = probeOf([null, '18:00:false'])

    expect(await waitForWorklogAnswered(stubStage(), 120, probe)).toBe(false)
    // Proves it really polled rather than bailing out on the rejection.
    expect(state.calls).toBeGreaterThan(2)
  })

  it('adopts the first successful reading as the baseline, then still sees a real change', async () => {
    // Same failed baseline, but the user answers a few polls later. The
    // recovered reading must have become the baseline, so the LATER change is
    // what resolves it — the fix must not overshoot into ignoring real answers.
    document.body.innerHTML = BUTTON
    const { probe } = probeOf([null, '18:00:false', '18:00:false', '21:30:true'])

    expect(await waitForWorklogAnswered(stubStage(), 2000, probe)).toBe(true)
  })

  it('resolves when the setting changes on a healthy read', async () => {
    document.body.innerHTML = BUTTON
    const { probe } = probeOf(['18:00:false', '18:00:false', '21:30:true'])

    expect(await waitForWorklogAnswered(stubStage(), 2000, probe)).toBe(true)
  })

  it('resolves when the dialog leaves the DOM with no settings change', async () => {
    // "Not now" / the backdrop on a replayed tour writes back identical values,
    // which no diff would ever see. The dialog closing is itself the answer.
    document.body.innerHTML = ''
    const { probe } = probeOf(['18:00:true'])

    expect(await waitForWorklogAnswered(stubStage(), 2000, probe)).toBe(true)
  })

  it('keeps waiting while nothing changes and the dialog is still up', async () => {
    document.body.innerHTML = BUTTON
    const { probe } = probeOf(['18:00:false'])

    expect(await waitForWorklogAnswered(stubStage(), 120, probe)).toBe(false)
  })

  it('does not resolve on reads that keep failing', async () => {
    // No baseline is ever established. Timing out is the correct answer; the
    // alternative — guessing — is what the fix exists to stop.
    document.body.innerHTML = BUTTON
    const { probe } = probeOf([null])

    expect(await waitForWorklogAnswered(stubStage(), 120, probe)).toBe(false)
  })
})
