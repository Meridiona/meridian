//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// While the walkthrough is waiting on one specific click, everything outside
// the spotlight must stop taking clicks.
//
// Before the fence, the only beat that protected itself was the dimmed one -
// and it did so by accident, because blocking was a side effect of the blur
// panes rather than anything anyone had asked for. Every other beat left the
// whole app live behind a decorative ring, including the close X on whatever
// modal the tour had just opened.
//
// That failure is silent and it strands the user. Clicking the X does not
// error: the modal closes, the tour goes on waiting for a click on a target
// that is no longer on screen, and the user reads an instruction about a
// window they cannot see until the beat's timeout expires - which is measured
// in minutes.
//
// Source-scanning: pointer-event routing across a stack of fixed-position
// layers is not something a headless run can resolve, and the failure is a
// click landing one layer too deep rather than an exception.

import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'fs'
import { join } from 'path'

const overlay = readFileSync(
  join(import.meta.dir, '..', 'components', 'tutorial', 'TutorialOverlay.tsx'), 'utf8',
)

describe('the walkthrough fences off stray clicks', () => {
  it('fences every spotlight beat, not only the dimmed one', () => {
    // One fence for every ringed beat. `spotlightDim` chooses whether it is
    // BLURRED, never whether it exists - the dimmed beat keeps its blur and
    // every other beat gets the same panes with nothing drawn on them.
    expect(overlay).toContain('{ring && <Cutout rect={ring} dim={spotlightDim} />}')
  })

  it('fences narration beats too, which have no ring to build panes around', () => {
    // THE REGRESSION THIS EXISTS FOR. `Cutout` fences everything AROUND a
    // target, so a beat with no target rendered no fence at all - and those are
    // most of the run. Worse, they are exactly the beats that narrate over a
    // modal the tour just opened (the Jira OAuth panel, the composer), so a
    // stray click hit Cancel or started a flow the tour was not expecting and
    // the walkthrough carried on describing a screen that no longer existed.
    // Reported from real runs twice.
    expect(overlay).toContain('{!ring && <FullFence />}')
    expect(overlay).toContain('function FullFence()')
  })

  it('builds both fences from one component', () => {
    // The blur and the fence are the same four panes. Two components would
    // drift, and the drift is invisible: the visible one would keep looking
    // right while the invisible one stopped covering the same area.
    expect(overlay).toContain('function Cutout({ rect, dim }')
    expect(overlay).not.toContain('function DimCutout')
    // Only the paint is conditional. Geometry and pointer-events are not.
    const at = overlay.indexOf('function Cutout({ rect, dim }')
    const pane = overlay.slice(at, overlay.indexOf('\n}', overlay.indexOf('return (', at)))
    expect(pane).toContain("pointerEvents: 'auto'")
    expect(pane).toMatch(/\.\.\.\(dim \? \{/)
  })

  it('does NOT gate the fence on `awaiting`', () => {
    // The inverse of what this test used to assert, and the whole bug.
    // `awaiting` is true only inside waitForClick / waitForValue /
    // waitForMinWords, so gating on it left the app live for every narration
    // beat. The old rationale was that fencing during narration would "feel
    // frozen for reasons the user cannot see" - but the scrim is the visible
    // reason, and Skip/choices/caption stay live as later siblings, so the way
    // out never depended on this.
    expect(overlay).not.toMatch(/awaiting && <Cutout/)
    expect(overlay).not.toMatch(/<Cutout[^>]*awaiting/)
  })

  it('still uses `awaiting` for the cue that it is the user\'s turn', () => {
    // Removing the gate must not remove the signal: the ring pulses only while
    // the tour is genuinely blocked on a click. That is what tells the user to
    // act - not whether some background panel happens to accept clicks.
    expect(overlay).toMatch(/animation: awaiting && ringLive \? 'mer-tour-pulse/)
  })

  it('keeps the way out clickable', () => {
    // The fence is rendered BEFORE the caption, the choice buttons and Skip, so
    // those later siblings paint over it and keep their own pointerEvents.
    // If the fence ever moves below them it swallows Skip, and a user who
    // cannot reach the target also cannot leave.
    const fence = overlay.indexOf('<Cutout rect={ring} dim={spotlightDim} />')
    // The RENDER site, not the prop. `indexOf('onSkip')` finds the destructuring
    // in the component signature ~340 lines earlier, which sits before the fence
    // no matter where the button is painted - so comparing against it would fail
    // a correct file and pass a broken one. Anchor on the JSX that actually
    // paints.
    const skip = overlay.indexOf('<button onClick={onSkip}')
    const caption = overlay.indexOf("pointerEvents: 'auto', maxWidth:")
    expect(fence).toBeGreaterThan(-1)
    expect(caption).toBeGreaterThan(fence)
    // Skip must paint AFTER the fence, not merely exist. The old `skip > -1` let
    // the fence move below the Skip button - the one arrangement this test is
    // here to forbid - without failing.
    expect(skip).toBeGreaterThan(fence)

    // The full-viewport fence needs the SAME guarantee, and needs it more: it
    // covers the entire screen, so if it ever painted over Skip the user would
    // have no way out of the walkthrough at all.
    const fullFence = overlay.indexOf('{!ring && <FullFence />}')
    expect(fullFence).toBeGreaterThan(-1)
    expect(caption).toBeGreaterThan(fullFence)
    expect(skip).toBeGreaterThan(fullFence)
  })
})
