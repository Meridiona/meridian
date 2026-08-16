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

describe('the walkthrough routes a blocked Generate to the AI setup', () => {
  const flow = scriptDay.slice(scriptDay.indexOf("s.spotlight('[data-tour=\"wl-open\"]')"))

  it('detours between the press and the draft, not after the tour', () => {
    // Order: they press Generate, they are told why it cannot run, they connect,
    // and only then does the draft arrive. An AI ask parked at the end of the
    // tour is an ask AFTER the feature it is required for has been demonstrated
    // working - which is the thing that made this wrong.
    const press = flow.indexOf("await s.waitForClick('[data-tour=\"wl-generate\"]'")
    const connect = flow.indexOf('wl-connect-provider')
    const draft = flow.indexOf("s.appeared('[data-tour=\"draft-targets\"]'")
    expect(press).toBeGreaterThan(-1)
    expect(connect).toBeGreaterThan(press)
    expect(draft).toBeGreaterThan(connect)
  })

  it('arms on the card actually appearing, never on a prediction', () => {
    // The same shape the composer's detour uses. Predicting from a flag would
    // strand the tour on both sides: a wait for a card that never came, or a
    // silent skip past one that did.
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

  it('comes back and presses Generate again', () => {
    // The draft dialog closes itself on the way to Settings, so returning to a
    // ringed `wl-generate` would ring a control that is no longer rendered.
    const connect = flow.indexOf('wl-connect-provider')
    const after = flow.slice(connect)
    expect(after.indexOf('wl-open')).toBeGreaterThan(-1)
    expect(after.indexOf('wl-open')).toBeLessThan(after.indexOf("s.appeared('[data-tour=\"draft-targets\"]'"))
  })

  it('checks the connect actually took, and does not claim it did', () => {
    // Settings here is UNLOCKED on purpose - the locked variant hands back to the
    // PLANNER on success, which is the wrong place to land from a worklog - so ×
    // is available throughout and closing it proves nothing. Someone who opens
    // the picker, finds their provider needs a CLI installed, and backs out comes
    // back to the same card.
    const connect = flow.indexOf('wl-connect-provider')
    const after = flow.slice(connect)
    // The re-probe exists, AFTER the second press.
    const rePress = after.indexOf("await s.waitForClick('[data-tour=\"wl-generate\"]'")
    expect(rePress).toBeGreaterThan(-1)
    // Searched from AFTER the press: `let blocked = …` (the first probe, before
    // the detour) contains this same text, so a plain indexOf would match that
    // one and pass on a file with no re-probe at all.
    const reProbe = after.indexOf('blocked = await s.appeared', rePress)
    expect(reProbe).toBeGreaterThan(rePress)
    // Nothing between the picker and that probe asserts a connection happened.
    expect(after.slice(0, reProbe)).not.toContain('Connected.')
  })

  it('puts the closing ask back when the connect did not take', () => {
    // Skipping it is right for someone who visited the picker and answered. It is
    // wrong once the tour can SEE that they did not - that user would otherwise
    // finish the walkthrough with a Meridian that cannot write anything, and
    // nothing having said so.
    expect(flow).toMatch(/if \(blocked\) aiState = null/)
  })

  it('does not narrate a draft that is not there', () => {
    // The beats that follow all describe a document: reading the work, a 90%
    // match, approve, posted. On a still-blocked dialog they narrate a screen
    // saying the opposite, then stall waiting for targets that never arrive.
    expect(flow).toMatch(/if \(!blocked\) \{/)
    const guard = flow.indexOf('if (!blocked) {')
    const narration = flow.indexOf("s.say('Reading the work")
    expect(narration).toBeGreaterThan(guard)
  })

  it('does not march them into the picker a second time at the end', () => {
    // The closing beat runs on "no evidence of a provider". Once the detour has
    // taken them to the picker, that is no longer true, and asking again reads
    // as the tour not having noticed what they just did.
    expect(scriptDay).not.toMatch(/^\s*if \(ai === null\) \{/m)
    expect(scriptDay).toMatch(/if \(aiState === null\) \{/)
    expect(scriptDay).toMatch(/aiState = 'connected-in-tour'/)
  })
})
