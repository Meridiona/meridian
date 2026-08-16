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
import { buildSteps, resumeIndex } from '@/app/setup/steps'

const page = readFileSync(join(import.meta.dir, '..', 'app', 'setup', 'page.tsx'), 'utf8')

describe('the setup wizard resumes where it left off', () => {
  it('stores the step ID, not its index', () => {
    expect(page).toContain('JSON.stringify({ stepId })')
    // Restoring resolves that id back to a position in the CURRENT list.
    expect(page).toContain('resumeIndex(steps, savedStepId)')
    // An id with no position to resume to starts from the top rather than guessing.
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

// A step that this build DROPS is not the same as a step it does not know, and
// `findIndex` alone could not tell them apart. Windows drops the Alerts step
// once notifications are granted - so the user who quits on that step, goes and
// grants notifications, and comes back is exactly the person whose saved
// position stopped resolving. Sending them to Welcome loses their place at the
// one moment they did what the wizard asked.
describe('resuming onto a step this run dropped', () => {
  const winDropped = buildSteps('windows', true) // notifications already granted
  const winFull = buildSteps('windows', false)
  const mac = buildSteps('darwin', false)

  it('the Alerts step really is dropped, or none of this is reachable', () => {
    // Guards the premise. If buildSteps stops dropping it, these cases become
    // vacuous rather than failing, and the whole block would quietly stop
    // testing anything.
    expect(winFull.some((s) => s.id === 'permissions')).toBe(true)
    expect(winDropped.some((s) => s.id === 'permissions')).toBe(false)
  })

  it('resumes past the dropped step instead of restarting at Welcome', () => {
    // -1 is the "stay on Welcome" signal in page.tsx. Anything >= 0 is a real
    // position, which is the whole point.
    expect(resumeIndex(winDropped, 'permissions')).toBeGreaterThanOrEqual(0)
  })

  it('and lands on the last step when nothing survives after it', () => {
    // Permissions is last, so "the step after it" does not exist - the honest
    // resume is the end of the wizard, whose footer offers Open Dashboard.
    expect(resumeIndex(winDropped, 'permissions')).toBe(winDropped.length - 1)
  })

  it('still starts from the top for an id this build has never heard of', () => {
    // The ORIGINAL behaviour, and the reason the id is stored rather than the
    // index. A step removed in an older build has no position to infer.
    expect(resumeIndex(mac, 'mlx-runtime')).toBe(-1)
    expect(resumeIndex(mac, '')).toBe(-1)
  })

  it('and resolves a present step to its own position, unchanged', () => {
    for (const steps of [mac, winFull, winDropped]) {
      steps.forEach((s, i) => expect(resumeIndex(steps, s.id)).toBe(i))
    }
  })
})
