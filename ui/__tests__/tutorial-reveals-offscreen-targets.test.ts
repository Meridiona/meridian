//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The tour must bring a target into view before it asks the user to click it.
//
// THE BUG: the composer's "Where" choice - keep the task personal, or file a
// real ticket your team can see - sits at the bottom of an `overflow-y-auto`
// panel in `TaskComposer`. On a short window it renders below the fold. The tour
// rang it, flew the cursor to it, said "Pick either - you can change it per
// task", and then sat on `waitForClick(..., 120000)` waiting for a click on
// radios the user could not see, let alone press. Two minutes later the beat
// timed out and moved on as though the choice had been made.
//
// That choice is the one decision in the composer with a consequence OUTSIDE
// Meridian - one option files a real ticket onto a shared board - so silently
// skipping past it is the worst beat in the script to lose.
//
// Nothing in the tutorial scrolled, anywhere. The engine measured targets with
// `getBoundingClientRect` and placed a ring at those coordinates, which is
// perfectly happy to draw a ring at a position no user can reach.
//
// Guarded in two halves, because the two failure modes are different:
//   1. the ENGINE's reveal rule, tested as a pure decision (below);
//   2. the CALL SITES - a correct rule nothing invokes is not a fix.

import { describe, it, expect, afterEach } from 'bun:test'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'

const root = join(import.meta.dir, '..')
const engineSrc = readFileSync(join(root, 'components/tutorial/engine.ts'), 'utf8')
const hookSrc = readFileSync(join(root, 'components/tutorial/useTutorial.tsx'), 'utf8')

// ── the reveal decision, exercised for real ──────────────────────────────────
//
// `revealTarget` reads the DOM and calls `scrollIntoView`, neither of which
// exists under bun. Both are stubbed so the DECISION - scroll, or leave it
// alone - can be asserted directly rather than scanned for.

type Box = { top: number; bottom: number; left: number; right: number }

const originals = {
  document: (globalThis as Record<string, unknown>).document,
  window: (globalThis as Record<string, unknown>).window,
}

/** Install a fake target with `box`, and return what `revealTarget` did. */
async function reveal(box: Box | null, viewport = { w: 1200, h: 800 }): Promise<boolean> {
  let scrolled = false
  const el = box && {
    getBoundingClientRect: () => ({
      ...box, width: box.right - box.left, height: box.bottom - box.top,
    }),
    scrollIntoView: () => { scrolled = true },
  }
  const g = globalThis as Record<string, unknown>
  // Only the TARGET selector resolves. `tourTarget` first looks up the tour
  // surface and, finding one, queries inside it - a stub that answers every
  // selector hands back a fake "surface" with no `querySelector` of its own.
  const target = '[data-tour="task-where"]'
  g.document = { querySelector: (sel: string) => (sel === target ? el ?? null : null) }
  g.window = { innerWidth: viewport.w, innerHeight: viewport.h }
  // Imported inside so the stubs are in place when the module reads them.
  const { revealTarget } = await import('../components/tutorial/engine')
  revealTarget('[data-tour="task-where"]')
  return scrolled
}

afterEach(() => {
  const g = globalThis as Record<string, unknown>
  g.document = originals.document
  g.window = originals.window
})

describe('the tour scrolls a target into view before pointing at it', () => {
  it('scrolls when the target is below the fold', async () => {
    // THE REPORTED CASE: the "Where" radios sit past the bottom edge of the
    // composer's scroller, so their rect is below the viewport.
    expect(await reveal({ top: 820, bottom: 910, left: 400, right: 800 })).toBe(true)
  })

  it('scrolls when the target is above the fold', async () => {
    expect(await reveal({ top: -40, bottom: 30, left: 400, right: 800 })).toBe(true)
  })

  it('leaves a fully visible target alone', async () => {
    // Scrolling unconditionally would make every beat twitch, including the
    // many that ring something already centred on screen.
    expect(await reveal({ top: 300, bottom: 360, left: 400, right: 800 })).toBe(false)
  })

  it('scrolls a target sitting flush against the edge', async () => {
    // Visible, and still not usable: the caption parks above or below the ring,
    // so a target 10px off the bottom has nowhere to put the sentence that
    // explains it. `REVEAL_MARGIN` is a required clearance for that reason,
    // rather than a plain on-screen test.
    expect(await reveal({ top: 300, bottom: 790, left: 400, right: 800 })).toBe(true)
  })

  it('does nothing when the target is missing or has no box', async () => {
    // Beats point at things that render conditionally (the "Where" section only
    // exists with a tracker connected). A missing target must be a no-op, not a
    // crash inside the tour's own step runner.
    expect(await reveal(null)).toBe(false)
    expect(await reveal({ top: 0, bottom: 0, left: 0, right: 0 })).toBe(false)
  })
})

describe('the reveal is actually wired into the tour', () => {
  // A correct rule nothing calls is not a fix, and this is exactly the shape of
  // guard that has passed vacuously before: assert the CALL, not the import.
  const body = (name: string) => {
    const at = hookSrc.indexOf(`      ${name}: `)
    expect(at, `${name} is gone from the stage API`).toBeGreaterThan(-1)
    const end = hookSrc.indexOf('\n      },', at)
    expect(end).toBeGreaterThan(at)
    return hookSrc.slice(at, end)
  }

  it('runs when a beat rings something', () => {
    expect(body('spotlight')).toContain('revealTarget(sel)')
  })

  it('runs when a beat points without ringing', () => {
    // `point` is used on its own in several beats. The cursor flying to a
    // coordinate off the bottom of a scroller is no more useful than a ring
    // drawn there.
    expect(body('point')).toContain('revealTarget(sel)')
  })

  it('centres rather than doing the minimum scroll', () => {
    // `block: 'nearest'` would satisfy "is it on screen" while leaving the
    // target flush against an edge - where the caption, which parks above or
    // below the ring, has nowhere sensible to go. The two constraints are not
    // the same and only centring meets both.
    expect(engineSrc).toContain("block: 'center'")
  })
})
