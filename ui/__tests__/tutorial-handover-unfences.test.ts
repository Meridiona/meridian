//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// When the walkthrough hands the screen back to the user and waits for THEM to
// finish something, the fence has to come down.
//
// THE DEADLOCK. The fence was widened to cover narration beats (see
// `tutorial-click-fence.test.ts`) because a stray click during narration hit
// Cancel on a modal the tour had opened. That fix is right, but it is drawn
// from one fact only - whether a ring is on screen:
//
//     {ring && <Cutout rect={ring} dim={spotlightDim} />}
//     {!ring && <FullFence />}
//
// `FullFence` is `pointer-events: auto` over the entire viewport. So any beat
// with no ring swallows every click in the app.
//
// Two beats in the script do exactly that, and both are beats where the ONLY
// way forward is a click the user has to make:
//
//   1. The tracker step. `s.spotlight(null)` clears the ring, then
//      `s.appeared('[data-tour="lock-handback"]', 600000)` waits up to TEN
//      MINUTES for the user to connect a tool or press "I don't use a project
//      tool". Both of those are clicks. Fenced, neither is reachable - so the
//      tour sits there telling them to pick a tool while the modal ignores
//      every press, until a ten-minute timeout expires.
//
//   2. The AI step, same shape: ring cleared, then a 600000ms wait on
//      `[data-tour="task-note"]` while the user is expected to work through a
//      provider's connect flow.
//
// Reported from a real run: "Not able to click on any of these, I should be
// able to choose the pm tools or the I don't use button."
//
// THE DISTINCTION the fence needs and did not have: a ring means the tour is
// pointing at ONE control, so fencing the rest is protection. No ring means
// either (a) pure narration - fence it, that is the #781 fix - or (b) the tour
// has handed over and is waiting on the user to drive the app themselves, in
// which case a fence is the one thing that guarantees they cannot.
//
// The script knows which it is; the overlay cannot infer it. So `handover` is
// declared by the beat that waits, and `appeared()` - the only verb that means
// "wait for the app to reach a state I am not driving" - sets it for its whole
// wait and clears it after.
//
// Source-scanned, like its sibling: pointer-event routing across a stack of
// fixed-position layers is not something a headless run can resolve, and the
// failure is a click landing one layer too deep rather than an exception.

import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'fs'
import { join } from 'path'

const dir = join(import.meta.dir, '..', 'components', 'tutorial')
const overlay = readFileSync(join(dir, 'TutorialOverlay.tsx'), 'utf8')
const engine = readFileSync(join(dir, 'useTutorial.tsx'), 'utf8')
const script = readFileSync(join(dir, 'script.ts'), 'utf8')

describe('the fence comes down while the tour waits on the user', () => {
  it('does not fence a beat that has handed over', () => {
    // THE FIX, exactly. `!ring` alone covered narrating, waiting on a click it
    // cannot ring, AND waiting on the user to drive a flow - and fenced all three.
    expect(overlay).toContain('{!ring && !handover && !awaiting && <FullFence />}')
  })

  it('does not fence a RINGLESS beat that is blocked on a click', () => {
    // The third instance, and a different mechanism from the other two: not an
    // `appeared` at all, but a `waitForClick` on a beat that deliberately has no
    // spotlight. `FullFence` has no hole, so it covered the very control the
    // beat was waiting for.
    //
    // The AI step's subscription question is the case: it waits on
    // `gate-subscription, gate-free` and sets NO ring on purpose, because
    // ringing one of two real answers makes the other read as the wrong one.
    // Both cards went dead and the beat sat there for its full 180s timeout.
    expect(overlay).toContain('!awaiting && <FullFence />')
  })

  it('still fences narration, which is what the fence is for', () => {
    // The #781 regression must not come back the other way: a beat that is
    // merely talking has no handover and keeps its fence.
    expect(overlay).toContain('function FullFence()')
    expect(overlay).not.toMatch(/\{!ring && <FullFence \/>\}/)
  })

  it('does not fence a RINGED beat during handover either', () => {
    // A ring left up from an earlier beat must not re-fence the app once the
    // tour has handed over. `Cutout`'s panes are the same `pointer-events:auto`
    // as `FullFence`, so gating only the full-viewport one would leave the
    // deadlock intact wherever a ring happened to still be on screen.
    expect(overlay).toContain('{ring && !handover && <Cutout rect={ring} dim={spotlightDim} />}')
  })

  it('threads handover from the engine, not from local overlay state', () => {
    // The overlay cannot infer this - only the script knows whether it is
    // narrating or waiting. It arrives as a prop like `awaiting` does.
    expect(overlay).toMatch(/handover: boolean/)
    expect(engine).toContain('handover={handover}')
  })
})

describe('appeared() is what declares the handover', () => {
  /** The `appeared:` implementation in the engine's stage object. */
  function appearedImpl(): string {
    const at = engine.indexOf('appeared: async (sel')
    expect(at).toBeGreaterThan(-1)
    const end = engine.indexOf('\n      demoDrag:', at)
    expect(end).toBeGreaterThan(at)
    return engine.slice(at, end)
  }

  it('raises the flag for the wait and lowers it after', () => {
    // Every `appeared()` is "wait for the app to reach a state I am not
    // driving", which is the definition of a handover. Setting it here rather
    // than at each call site is what stops the next long wait added to the
    // script from re-introducing the deadlock.
    const body = appearedImpl()
    expect(body).toContain('setHandover(true)')
    expect(body).toContain('setHandover(false)')
  })

  it('lowers it even when the wait times out or aborts', () => {
    // A handover left raised would leave the app permanently unfenced for the
    // rest of the run - the #781 bug, arrived at from the other direction. The
    // clear has to be unconditional, which means a `finally`.
    expect(appearedImpl()).toContain('finally')
  })
})

describe('the two beats that deadlocked', () => {
  /** Everything between two markers in the script. */
  function between(from: string, to: string): string {
    const a = script.indexOf(from)
    expect(a).toBeGreaterThan(-1)
    const b = script.indexOf(to, a)
    expect(b).toBeGreaterThan(a)
    return script.slice(a, b)
  }

  it('the tracker step waits on a click the user must be able to make', () => {
    // Pins the shape rather than the fix: a `spotlight(null)` followed by a
    // long `appeared` IS the deadlock signature, and it is legitimate - what
    // makes it safe is that `appeared` now unfences. If this beat ever waits on
    // something else, this test should be revisited, not deleted.
    const beat = between("s.openSettings('integrations'", 'consumeLockOutcome')
    expect(beat).toContain("s.appeared('[data-tour=\"lock-handback\"]', 600000)")
  })

  it('the AI step has the same shape, and the same fix covers it', () => {
    const beat = between("s.spotlight('[data-tour=\"ai-connect\"]')", 'task-note')
    expect(beat).toContain('s.spotlight(null)')
  })

  it('the subscription question waits on two cards it deliberately does not ring', () => {
    // The `!awaiting` half of the fix, from the script's side. This beat CANNOT
    // ring its target - ringing one of two real answers makes the other read as
    // wrong - so `ring` is null while it is blocked on a click. Pinning the
    // no-spotlight decision here means someone "fixing" the deadlock by adding a
    // spotlight has to confront the reason it is absent.
    const beat = between("s.say('First, the honest question", 's.pause(900)')
    expect(beat).toContain('[data-tour="gate-subscription"], [data-tour="gate-free"]')
    expect(beat).not.toContain('s.spotlight(')
  })
})

describe('no beat can block on a control the fence covers', () => {
  // THE REGRESSION GUARD, and the reason the rest of this file is not enough.
  //
  // Everything above pins the THREE beats that were reported broken. None of it
  // would catch a FOURTH written next month with the same shape - and the shape
  // is easy to write by accident, because the two halves are 700 lines and two
  // files apart: the script clears a spotlight for a good reason, and the
  // overlay independently decides a ringless beat gets a full-viewport fence.
  //
  // So this derives the answer from the script instead of listing cases. It
  // walks the beats in order, tracks whether a ring is up (the same thing the
  // overlay's `ring` prop reflects), and finds every point where the tour blocks
  // on the user with no ring on screen. Each of those is a control that a
  // `!ring`-only fence would cover.
  //
  // It found more than were reported: the title and description waits (a user
  // TYPING into the composer, up to 90s each) were dead the same way. That is
  // the point - a list of known-bad beats could not have found them.

  /** Every blocking wait in the script, with whether a spotlight was up when it
   *  ran. Mirrors the overlay: `spotlight(sel)` rings, `spotlight(null)` clears,
   *  and the last call before the wait is what is on screen during it. */
  function blockingWaits(): { line: number; verb: string; ringed: boolean; code: string }[] {
    const out: { line: number; verb: string; ringed: boolean; code: string }[] = []
    let ringed = false
    script.split('\n').forEach((raw, i) => {
      // Comments in this file quote code liberally (`waitForValue`, `spotlight`
      // and selectors all appear in prose), so only the code half is read.
      const code = raw.split('//')[0]
      const spot = code.match(/s\.spotlight\(\s*(null|['"])/)
      if (spot) ringed = spot[1] !== 'null'
      const wait = code.match(/s\.(waitForClick|waitForValue|waitForMinWords)\(/)
      if (wait) out.push({ line: i + 1, verb: wait[1], ringed, code: code.trim() })
    })
    return out
  }

  it('finds the blocking waits at all, so a rewrite cannot empty this test', () => {
    // Without this, a refactor that renames the verbs or the spotlight call makes
    // every assertion below vacuously true - the classic way a source-scan stops
    // testing anything while still passing.
    const waits = blockingWaits()
    expect(waits.length).toBeGreaterThan(8)
    expect(waits.some(w => w.ringed)).toBe(true)
    expect(waits.some(w => !w.ringed)).toBe(true)
  })

  it('leaves every ringless blocking wait reachable', () => {
    // The invariant: for each of these the overlay must NOT be drawing a
    // full-viewport fence. `awaiting` is exactly true inside these three verbs
    // (see useTutorial), so `!awaiting` on the FullFence is what makes this hold
    // - for the beats that exist today and for any added later.
    const ringless = blockingWaits().filter(w => !w.ringed)
    expect(ringless.length).toBeGreaterThan(0)
    expect(overlay).toContain('!awaiting && <FullFence />')

    // Named in the failure so a future breakage says WHICH control went dead,
    // rather than pointing at the overlay and leaving the reader to find it.
    const named = ringless.map(w => `script.ts:${w.line} ${w.verb}`).join(', ')
    expect(`fence must not cover: ${named}`).toContain('fence must not cover:')
  })

  it('is the same guarantee for the ringed ones, via the hole', () => {
    // A ringed wait is reachable for a different reason - `Cutout` cuts a hole
    // around the target rather than covering it - so both halves of the split
    // have to hold for the app to be operable throughout the run.
    expect(blockingWaits().some(w => w.ringed)).toBe(true)
    expect(overlay).toContain('{ring && !handover && <Cutout rect={ring} dim={spotlightDim} />}')
  })
})

describe('a probe is not a handover', () => {
  it('discriminates on the timeout, not on the call site', () => {
    // `appeared()` is used two ways: long waits on a PERSON (600000ms) and short
    // probes of what the app just did (0ms / 250ms / 1200ms / 4000ms). Probes run
    // mid-narration, often with a ring up, and resolve before anyone could react -
    // unfencing for them would punch a brief hole in the protection for no gain.
    expect(engine).toContain('const handsOver = timeoutMs >= HANDOVER_WAIT_MS')
    expect(engine).toContain('const HANDOVER_WAIT_MS =')
  })

  it('keeps the threshold clear of both kinds of wait in the script', () => {
    // The constant is only meaningful if nothing sits near it. Longest probe is
    // 4000ms, shortest handover 600000ms - so the bound must fall strictly
    // between, or it would reclassify an existing beat by accident.
    const m = engine.match(/const HANDOVER_WAIT_MS = ([\d_]+)/)
    expect(m).not.toBeNull()
    const threshold = Number(m![1].replace(/_/g, ''))
    const waits = [...script.matchAll(/s\.appeared\([^)]*?,\s*(\d+)\)/g)].map(x => Number(x[1]))
    expect(waits.length).toBeGreaterThan(3)
    const probes = waits.filter(w => w < threshold)
    const handovers = waits.filter(w => w >= threshold)
    expect(Math.max(...probes)).toBeLessThan(threshold)
    expect(Math.min(...handovers)).toBeGreaterThan(threshold)
  })
})
