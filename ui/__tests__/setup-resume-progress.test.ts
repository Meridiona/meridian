//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The setup wizard resumes where the user left off after a quit.
//
// Three properties matter here, and two of them are the kind that break
// silently - the wizard still opens, just on the wrong screen, which reads as
// "it forgot" rather than as a bug:
//
// 1. Progress is keyed by step ID, never by index. `buildSteps` is
//    platform-dependent and the list changes between builds, so an index is
//    only meaningful against the exact list that produced it. Restoring `2`
//    against a different list lands on a different step with no error.
// 2. Restore waits for `platform` to resolve. page.tsx already documents why:
//    the step list must never change shape underneath a `step` index that has
//    been navigated to. Restoring early reintroduces that bug from the other
//    side.
// 3. Finishing clears the key, or Re-run Setup opens on the last step of the
//    run that just ended.

import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'fs'
import { join } from 'path'

const page = readFileSync(join(import.meta.dir, '..', 'app', 'setup', 'page.tsx'), 'utf8')

describe('the setup wizard resumes where it left off', () => {
  it('stores the step ID, not its index', () => {
    expect(page).toContain('JSON.stringify({ stepId })')
    // Restoring resolves that id back to a position in the CURRENT list.
    expect(page).toContain('steps.findIndex((s) => s.id === savedStepId)')
    // An id that no longer exists starts from the top rather than guessing.
    expect(page).toMatch(/if \(at < 0\) return/)
  })

  it('waits for every step-list input before restoring', () => {
    // The guard that keeps the step list from reshaping under a restored
    // index - the same invariant the Welcome screen holds for forward
    // navigation. It must cover EVERY input to buildSteps, not just the
    // platform: the notifications precheck can drop a step, so restoring
    // before it lands could point `step` into a list about to shrink.
    expect(page).toMatch(
      /if \(restoredProgress\.current \|\| platform === null \|\| notifPrecheck === null\) return/,
    )
    // …and the writer is gated on it too, so a null-platform render cannot
    // persist a position derived from the wrong list.
    expect(page).toMatch(/if \(welcome \|\| platform === null\) return/)
  })

  it('clears progress when setup finishes', () => {
    const at = page.indexOf('const finish = async () => {')
    if (at < 0) throw new Error('marker not found: const finish')
    const end = page.indexOf('\n  }', at)
    expect(page.slice(at, end)).toContain('localStorage.removeItem(PROGRESS_KEY)')
  })

  it('never lets a storage failure take the wizard down', () => {
    // Private mode throws on localStorage access. The wizard opening is worth
    // more than remembering a position, so every touch is guarded - a throw
    // here would blank the whole setup window.
    const touches = page.match(/localStorage\.(getItem|setItem|removeItem)/g) ?? []
    const guarded = page.match(/try \{[^}]*localStorage\.(getItem|setItem|removeItem)/g) ?? []
    expect(touches.length).toBeGreaterThan(0)
    expect(guarded.length).toBe(touches.length)
  })
})
