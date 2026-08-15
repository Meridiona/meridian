//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The first-run walkthrough must have a way out.
//
// It used to have none, on purpose: `onSkip={explicit ? finish : null}` gave the
// control to replays only, on the argument that an escape hatch on the first
// screen a new user ever sees reads as the recommended action to anyone unsure -
// which is everyone at that moment.
//
// That argument assumed the tour only ever arrives when the user has just asked
// for the product to explain itself. It does not: a tour armed by one path and
// delivered on a later dashboard mount lands on someone who did not ask for it
// and, with no Skip, cannot leave it. The only exit was closing the window. The
// decision is reversed deliberately - see the comment at the render site.
//
// Source-scanning: whether a control renders for a `null` vs non-`null` prop on
// a full-screen overlay is a wiring question, and the failure mode is a missing
// button rather than an exception.

import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'fs'
import { join } from 'path'

const dir = join(import.meta.dir, '..', 'components', 'tutorial')
const hook = readFileSync(join(dir, 'useTutorial.tsx'), 'utf8')
const overlay = readFileSync(join(dir, 'TutorialOverlay.tsx'), 'utf8')

describe('the walkthrough can always be left', () => {
  it('hands the overlay a skip on every run, not only replays', () => {
    expect(hook).toContain('onSkip={finish}')
    // The reversed form. If this comes back, the first-run tour is a trap again.
    expect(hook).not.toContain('onSkip={explicit ? finish : null}')
  })

  it('renders the control whenever it is given one', () => {
    expect(overlay).toContain('{onSkip && (')
  })

  it('keeps the skip in a fixed spot for the whole run', () => {
    // It lives outside the caption bubble on purpose: inside, it vanished on
    // any beat with an empty caption and slid sideways as the caption resized.
    // An escape hatch that moves, or disappears exactly when a confused user
    // reaches for it, is not an escape hatch.
    const at = overlay.indexOf('{onSkip && (')
    expect(at).toBeGreaterThan(-1)
    expect(overlay.slice(at, at + 400)).toContain('top: 14, right: 16')
  })

  it('pays off the entitlement when the tour ends, however it ends', () => {
    // `finish` runs on completion, on skip, and on an unexpected throw. All
    // three mean the tour is done, and the tray - which cannot read the
    // frontend's localStorage marker - learns that only from this call.
    const at = hook.indexOf('const finish = useCallback(')
    expect(at).toBeGreaterThan(-1)
    const body = hook.slice(at, hook.indexOf('}, [setActiveModal])', at))
    expect(body).toContain("invoke('disarm_walkthrough')")
    // Next to the localStorage write, so the two answers to "has this user
    // taken the tour" cannot drift apart.
    expect(body).toContain('localStorage.setItem(SEEN_KEY')
  })
})
