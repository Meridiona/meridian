//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// What the worklog dialog does when the update landed on a PERSONAL task.
//
// Two bugs this covers, both of which made the local case borrow a shape built
// for a real ticket:
//
// 1. The "GOES TO" row was a button on every provider, including `local`. There
//    is no ticket to open, so it raised the task dialog UNDERNEATH this one -
//    the click looked like it did nothing, and what it opened was found only
//    after closing the draft.
// 2. The posted footer said "Posted to LOCAL-9" with a link chip and offered to
//    draft a FOLLOW-UP. Nothing had been posted anywhere, and the one thing
//    worth doing next - putting the work on a real ticket - was on another
//    surface. It now offers create/match, or, with no tracker connected, says
//    that plainly instead of offering two buttons that can only fail.
//
// Scanned from source: this repo has no React render harness (see
// task-composer.test.ts for the convention).

import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'

const src = (p: string) => readFileSync(join(import.meta.dir, '..', p), 'utf8')
const targets = src('components/timeline/WorklogTargets.tsx')
const actions = src('components/timeline/WorklogActions.tsx')
const dialog = src('components/timeline/WorklogDraftDialog.tsx')
const hook = src('components/timeline/useWorklog.ts')
const demo = src('components/tutorial/demoWorklog.ts')

describe('a personal target row', () => {
  it('is not a button, so it cannot open a dialog behind this one', () => {
    // The tag itself is chosen from `isPersonal`, and the handler is dropped with
    // it - either alone would leave the other half of the bug in place (a div
    // that still calls onOpen, or a button that silently does nothing).
    expect(targets).toMatch(/const Row = isPersonal \? 'div' : 'button'/)
    expect(targets).toMatch(/onClick=\{isPersonal \? undefined : onOpen\}/)
  })

  it('no longer tells the user to open it', () => {
    expect(targets).not.toMatch(/Open it to create a ticket/)
  })
})

describe('the posted footer for a personal task', () => {
  it('picks the personal bar only when every posted target is local', () => {
    // `some` here would show the escalation controls on a draft that also posted
    // to a real ticket, where the work IS on the board already.
    expect(dialog).toMatch(/done\.every\(\(t\) => t\.provider === LOCAL_PROVIDER\)/)
    expect(dialog).toMatch(/posted && personalPost \?/)
  })

  it('offers both ways onto a real ticket when a tracker is connected', () => {
    expect(dialog).toMatch(/escalate_personal_task_create/)
    expect(dialog).toMatch(/escalate_personal_task_match/)
    expect(actions).toMatch(/Match to existing ticket/)
  })

  it('asks for a tracker instead when there is none', () => {
    // The branch is on the connected count, not on a flag that could be stale:
    // two escalate buttons with nowhere to escalate to can only fail.
    expect(actions).toMatch(/const connected = trackers\.length > 0/)
    expect(actions).toMatch(/Connect a tracker to put this on a ticket/)
  })

  it('does not offer a follow-up draft, which claimed the work was filed', () => {
    const personalBar = actions.slice(
      actions.indexOf('export function PersonalPostedBar'),
      actions.indexOf('export function PostedBar'),
    )
    expect(personalBar.length).toBeGreaterThan(0)
    expect(personalBar).not.toMatch(/follow-up/i)
  })
})

describe('after escalating', () => {
  it('re-reads the draft rather than patching it locally', () => {
    // Escalation repoints day_task_worklog_targets at the real ticket inside the
    // daemon. Reconstructing that here would be a second implementation of the
    // same rules, free to drift from it.
    expect(dialog).toMatch(/wl\.refresh\(\)/)
    expect(hook).toMatch(/function runRefresh/)
    expect(hook).toMatch(/refresh: \(\) => runRefresh\(day, taskId\)/)
  })

  it('refuses to stomp an in-flight generate or approve', () => {
    const fn = hook.slice(hook.indexOf('function runRefresh'), hook.indexOf('// ── The hook'))
    expect(fn).toMatch(/phase === 'generating' \|\| .*phase === 'approving'/)
  })

  it('is a no-op in the walkthrough, which owns its own scripted draft', () => {
    expect(demo).toMatch(/refresh: \(\) => \{\}/)
  })
})
