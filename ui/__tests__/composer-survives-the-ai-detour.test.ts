//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'fs'
import {
  armResume, consumeResume, isResumePending, resetComposer,
} from '@/components/plan/useTaskComposer'

// Pressing "Draft with AI" with no provider connected sends the user to Settings,
// which takes the shell's SINGLE modal slot - so the plan modal, and everything
// inside it, unmounts. Coming back is the same flow continuing.
//
// Two things have to survive that, and only one of them did. The note survived,
// because the composer's state is a module store. The fact that the composer was
// OPEN did not: `PlanBoardColumn` holds that in `useState`, which died with the
// unmount, so the user was handed back the board list and had to press
// "+ New task" again to find the work they had already done. Mid-walkthrough it
// also stranded the tour - the next beat points at fields that only exist inside
// the composer, and they were not on screen.

const uiRoot = import.meta.dir + '/..'
const readSrc = (rel: string): string => readFileSync(uiRoot + '/' + rel, 'utf8')

describe('the composer reopens itself after the AI-provider detour', () => {
  it('peeking does not spend the flag the composer needs', () => {
    // THE failure mode this split exists to avoid. `TaskComposer`'s mount effect
    // reads `consumeResume()` to decide whether to keep the note or reset it. If
    // the column consumed the flag to decide what to render, the composer would
    // mount, find nothing armed, treat it as an ordinary open, and wipe the very
    // note the column had just reopened to show.
    resetComposer()
    armResume()

    expect(isResumePending()).toBe(true)
    expect(isResumePending()).toBe(true) // still armed - peeking is not spending

    expect(consumeResume()).toBe(true)
    expect(isResumePending()).toBe(false)
    expect(consumeResume()).toBe(false) // one-shot, as before
  })

  it('is false on an ordinary open, so the column stays on the board list', () => {
    resetComposer()
    consumeResume() // drain anything a previous test armed
    expect(isResumePending()).toBe(false)
  })

  it('the board column opens the composer from it', () => {
    const src = readSrc('components/plan/PlanBoardColumn.tsx')
    // Passed as the initializer, not called into `useState(isResumePending())`:
    // the latter re-evaluates on every render, and React would ignore it after
    // the first anyway - so it reads as doing something it does not do.
    expect(src).toContain('useState(isResumePending)')
    expect(src).not.toContain('useState(false)')
  })

  it('and the shell still arms it only for the AI lock', () => {
    // The TRACKER lock is reached from the walkthrough's team-or-solo question,
    // BEFORE the composer has been opened at all. Arming resume there would now
    // do more than resurrect a stale note - it would pop the composer open over
    // the board list on a step that never asked for it.
    const shell = readSrc('components/timeline/MeridianTimelineShell.tsx')
    expect(shell).toContain("if (settingsSection !== 'integrations') armResume()")
  })
})
