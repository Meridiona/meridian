//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Drafting a worklog needs a model. Two things stood between a user and that
// fact, and they compounded.
//
// 1. THE WAY OUT WAS INVISIBLE. Press Generate with no provider and the dialog
//    answers with "Connect an AI provider ... Choose a provider". That button
//    calls `onOpenSettings`, which opens `SettingsModal` inside `ModalShell` at
//    `absolute inset-0 z-40` - while the draft dialog the user is still looking
//    at is `fixed inset-0 z-50`, portalled to `document.body`. The shell's root
//    is `relative` with no z-index, so it opens no stacking context and the two
//    compete directly: z-50 wins. Settings opened BEHIND the dialog. Nothing
//    appeared to happen, which is the worst possible answer to "how do I fix
//    this?".
//
// 2. THE WALKTHROUGH SKIPPED THE CHECK ENTIRELY. `attemptGenerate` returned
//    early on `isDemo`, so the first-run tour demonstrated Generate producing a
//    finished worklog on a machine with no AI set up at all - teaching a flow
//    that cannot work, to precisely the audience least able to tell.
//
// Source-scanning: both failures are a surface not appearing. There is no
// exception, no rejected promise and no state to read - the assertion a unit
// test would make ("Settings opened") is true in the DOM in the broken case
// too. What can be checked is the wiring that decides it.

import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'

const read = (...p: string[]) => readFileSync(join(import.meta.dir, '..', ...p), 'utf8')

const dialog = read('components', 'timeline', 'WorklogDraftDialog.tsx')
const actions = read('components', 'timeline', 'WorklogActions.tsx')
const scriptDay = read('components', 'tutorial', 'scriptDay.ts')

describe('the way out of a blocked worklog draft is visible', () => {
  it('closes the draft before sending the user to Settings', () => {
    // The order is the whole fix. `onOpenSettings` alone leaves a z-50 dialog
    // sitting on top of the z-40 modal it just opened.
    const at = dialog.indexOf('onConnectProvider=')
    expect(at).toBeGreaterThan(-1)
    const handler = dialog.slice(at, at + 200)
    expect(handler).toContain('onClose()')
    expect(handler).toContain("onOpenSettings('intelligence')")
    expect(handler.indexOf('onClose()')).toBeLessThan(handler.indexOf("onOpenSettings('intelligence')"))
  })

  it('gives the connect button a tour hook', () => {
    // The walkthrough rings this button, so it needs a stable handle. Without
    // one the tour's detour has nothing to point at and stalls on a timeout.
    const at = actions.indexOf('Choose a provider')
    expect(at).toBeGreaterThan(-1)
    const button = actions.slice(Math.max(0, at - 500), at)
    expect(button).toContain('data-tour="wl-connect-provider"')
  })
})

describe('the walkthrough does not demo a draft the user cannot get', () => {
  it('runs the real provider check even when the worklog is scripted', () => {
    // The bypass. `isDemo` skipping straight to `generate()` is what let the
    // tour show a finished worklog on a machine with no model.
    //
    // Removing it is safe for the tour because the control does NOT get swapped
    // out - the button keeps its name and the reason appears underneath it - so
    // a beat ringing `wl-generate` still has something to ring.
    const at = dialog.indexOf('const attemptGenerate')
    expect(at).toBeGreaterThan(-1)
    const fn = dialog.slice(at, dialog.indexOf('\n  }', at))
    expect(fn).not.toMatch(/if \(isDemo\)/)
    expect(fn).toContain("load<HealthStatus>('/api/health', 'get_health')")
    expect(fn).toContain('llm_provider_ok === false')
  })

  it('still fails OPEN when the probe itself fails', () => {
    // A health read that does not come back must not block drafting. That
    // failure has nothing to do with the model and is strictly worse than the
    // dead end it would prevent - and it is now the tour's safety net too: no
    // card appears, so the detour never arms and the tour carries on.
    const at = dialog.indexOf('const attemptGenerate')
    const fn = dialog.slice(at, dialog.indexOf('\n  }', at))
    const rescue = fn.slice(fn.indexOf('} catch {'))
    expect(rescue).toContain('setProviderDown(false)')
    expect(rescue).toContain('generate()')
  })
})

describe('the walkthrough leans on the AI step it already has', () => {
  // BOUNDED to the worklog stretch. Slicing to the end of the file would sweep in
  // beat 9 - the closing AI ask - whose `openSettings` is exactly the thing this
  // block is here to say should happen ONCE, and somewhere else.
  const flowStart = scriptDay.indexOf("s.spotlight('[data-tour=\"wl-open\"]')")
  const flowEnd = scriptDay.indexOf('── 6. The daily summary', flowStart)
  if (flowStart < 0 || flowEnd < flowStart) throw new Error('worklog stretch not found')
  const flow = scriptDay.slice(flowStart, flowEnd)

  it('runs no second connect flow of its own', () => {
    // "Draft with AI" in part one needs the SAME provider and raises the SAME
    // connect card (`ai-connect`), and it happens first - on the user's own task,
    // at the moment the requirement first means anything. A second Settings trip
    // here teaches that these are two separate AI features with two setups,
    // which is the opposite of true.
    expect(flow).not.toContain("s.openSettings('intelligence')")
    // …and it does not ring the connect card either. DETECTING it is the point;
    // making it the start of another picker walk is not.
    expect(flow).not.toMatch(/spotlight\('\[data-tour="wl-connect-provider"\]'\)/)
  })

  it('names the model as the one they already connected', () => {
    // Without this the beat reads as a second, separate AI feature - and a user
    // who found connecting one a chore braces for another round of it.
    const press = flow.indexOf("await s.waitForClick('[data-tour=\"wl-generate\"]'")
    const same = flow.toLowerCase().indexOf('same ai')
    expect(same).toBeGreaterThan(-1)
    expect(same).toBeLessThan(press)   // said BEFORE the press, not after it
  })

  it('arms on the card actually appearing, never on a prediction', () => {
    // Predicting from what part one observed strands the tour on both sides: a
    // wait for a card that never came, or a silent skip past one that did.
    expect(flow).toMatch(/await s\.appeared\('\[data-tour="wl-connect-provider"\]'/)
  })

  it('waits for EITHER outcome rather than a window sized for the card', () => {
    // The card and the draft hang off the same health read. A fixed wait long
    // enough to catch the card parks that many silent seconds in front of every
    // user who is already set up - and if their draft lands inside it, the line
    // after this narrates work that has already finished.
    const both = flow.indexOf('[data-tour="wl-connect-provider"], [data-tour="draft-targets"]')
    expect(both).toBeGreaterThan(-1)
    // …and the which-one probe comes after it, cheap rather than another wait.
    const probe = flow.indexOf('\'[data-tour="wl-connect-provider"]\', 250')
    expect(probe).toBeGreaterThan(both)
  })

  it('does not narrate a draft that is not there', () => {
    // The beats that follow all describe a document: reading the work, a 90%
    // match, approve, posted. On a blocked dialog they narrate a screen saying
    // the opposite, then stall waiting for targets that never arrive.
    expect(flow).toMatch(/if \(!blocked\) \{/)
    const guard = flow.indexOf('if (!blocked) {')
    const narration = flow.indexOf("s.say('Reading the work")
    expect(narration).toBeGreaterThan(guard)
  })

  it('points a blocked user back at the button they already met', () => {
    // Not a fresh instruction - a pointer to the step they have already been
    // through. That is what makes it one requirement rather than two.
    const alt = flow.slice(flow.indexOf('} else {', flow.indexOf('if (!blocked) {')))
    expect(alt).toContain('Draft with AI')
  })

  it('leaves the closing ask exactly where it was', () => {
    // Part two changes nothing about what we know, so the gate stays `ctx.ai`.
    // That closing beat is now the ONE place a user who declined in part one is
    // asked again - which is the right number of times.
    expect(scriptDay).toMatch(/if \(ai === null\) \{/)
    expect(scriptDay).not.toMatch(/\baiState\b/)
  })
})
