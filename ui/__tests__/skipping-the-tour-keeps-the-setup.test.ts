//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'fs'

// What still gets asked when someone presses "Skip tour".
//
// The walkthrough teaches, it does not configure - every REQUIRED setting is
// asked somewhere the tour cannot swallow. Permissions, sign-in, integrations
// and the AI provider are wizard steps (`app/setup/steps.tsx`), reached before
// the tour exists; a provider that is selected but not working is then carried
// by HealthBanner's `llm_provider_ok === false` for as long as it stays true.
//
// The worklog delivery time is the ONE exception, and the only thing here worth
// a test: it is asked at the tour's closing beat, so a skip before that beat
// would lose it outright if the shell's own trigger did not survive. It does -
// the gate defers on `!tutorial.running` rather than dropping - and these tests
// pin both directions of that, because the two are one line apart and a change
// to either reads as harmless.

const uiRoot = import.meta.dir + '/..'
const readSrc = (rel: string): string => readFileSync(uiRoot + '/' + rel, 'utf8')

describe('skipping the walkthrough does not skip the setup', () => {
  const shell = readSrc('components/timeline/MeridianTimelineShell.tsx')

  it('the worklog-time ask is DEFERRED during the tour, not dropped', () => {
    // `!tutorial.running` in the render gate, never a `setShowWorklogPrompt(false)`
    // when the tour starts. The distinction is the whole fallback: deferring means
    // the question returns the frame the tour ends, however it ended.
    expect(shell).toContain('{showWorklogPrompt && !activeModal && !openTask && !tutorial.running && (')
    // The latch is set from settings and cleared only by answering.
    expect(shell).toContain('setShowWorklogPrompt(true)')
  })

  it('and is not asked twice when the tour DID reach that beat', () => {
    // Both copies write the same setting, so answering either answers the
    // question - but `worklogPrompted` is read once on mount and cannot notice
    // the write, so the tour's copy has to clear the shell's latch by hand.
    // Without this, finishing the tour re-raised the identical dialog.
    const tourCopy = shell.slice(shell.indexOf('{tourWorklogSchedule && ('))
    const body = tourCopy.slice(0, tourCopy.indexOf('/>'))
    expect(body).toContain('setTourWorklogSchedule(false)')
    expect(body).toContain('setShowWorklogPrompt(false)')
  })

  it('the AI provider is a wizard step, so the tour is not its only door', () => {
    const steps = readSrc('app/setup/steps.tsx')
    expect(steps).toContain("id: 'provider'")
  })

  it('and a provider that stops working keeps saying so after a skip', () => {
    // The tour's gate is not the only thing that notices a dead provider - this
    // banner reads live health, so skipping past the gate leaves the user with a
    // standing, dismissible-but-returning notice rather than silence.
    const banner = readSrc('components/HealthBanner.tsx')
    expect(banner).toContain('health.llm_provider_ok === false')
  })

  it('finish() is the skip path AND the completion path', () => {
    // Both routes run the same teardown, which is why the deferral above is
    // enough for both: there is no skip-only exit that could forget to release
    // the deferred dialog.
    const hook = readSrc('components/tutorial/useTutorial.tsx')
    expect(hook).toContain('onSkip={finish}')
    expect(hook).toContain('setWalkthroughRunning(false)')
  })
})
