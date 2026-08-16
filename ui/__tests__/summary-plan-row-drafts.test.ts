//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Guards the click-through on TODAY'S PLAN rows in the daily summary.
//
// THE BUG: a plan row with tracked work behind it - every "Done · 1h 34m" row in the
// panel - was rendered with `onClick === undefined`, so `PlanRow` emitted a `<div>`
// rather than a `<button>`. Clicking the finished half of your day did NOTHING. The
// worklog draft for that work existed, was reachable from the "not on the plan" rows
// through exactly the same `onSelect` handler, and had a full edit/approve/post flow
// waiting behind it (`SummaryTaskView`) - it just had no entry point from the plan.
//
// Only the UNTOUCHED tickets led anywhere, and they led to the ticket. So the rows
// that led somewhere were the ones with no work to file, and the rows with a draft
// waiting to be approved and posted were inert.
//
// The file's own header had described the intended behaviour all along ("the first
// workstream behind it ... supplies the title and the click-through to the worklog
// flow"), so the code was the half that disagreed with the design, not the docs.
//
// Two things are pinned here, because each failed independently:
//   1. A plan row with work opens that work (onSelect), not the ticket.
//   2. `drafts` reaches the plan rows at all, so DRAFT READY TO POST can show on a
//      committed ticket - it previously rendered only under "not on the plan".
//
// No React render harness in this repo (see plan-store / oauth-setup-lifecycle), so
// the destination logic is unit-tested through the exported `pickPrimary`, and the
// wiring is source-scanned.

import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'fs'
import { pickPrimary } from '../components/summary/WorkList'
import type { DayTask } from '../lib/api-types'

const uiRoot = import.meta.dir + '/..'
const src = readFileSync(uiRoot + '/components/summary/WorkList.tsx', 'utf8')

/** A day task with only the fields `pickPrimary` reads. */
function task(id: string, minutes: number): DayTask {
  return { id, title: `task ${id}`, minutes } as DayTask
}

/** The `plan.map(...)` body - where a row's destination and badge are decided. */
function planRowWiring(): string {
  const at = src.indexOf('{plan.map(')
  expect(at).toBeGreaterThan(-1)
  const end = src.indexOf('</ul>', at)
  expect(end).toBeGreaterThan(at)
  return src.slice(at, end)
}

describe('which workstream a plan row opens', () => {
  it('picks the longest, not whichever id the join emitted first', () => {
    const byId = new Map([
      ['a', task('a', 20)],
      ['b', task('b', 95)],
      ['c', task('c', 40)],
    ])
    // 'a' is first in the list; 'b' is the biggest slice of the row's duration.
    expect(pickPrimary({ day_task_ids: ['a', 'b', 'c'] }, byId)?.id).toBe('b')
  })

  it('skips ids with no matching task instead of counting them as empty', () => {
    // A verdict can name a strand that fell below the day floor and is absent
    // from `tasks`; that must not become the row's destination.
    const byId = new Map([['real', task('real', 10)]])
    expect(pickPrimary({ day_task_ids: ['ghost', 'real'] }, byId)?.id).toBe('real')
  })

  it('is undefined for a ticket with no tracked work, so the row opens the ticket', () => {
    expect(pickPrimary({ day_task_ids: [] }, new Map())).toBeUndefined()
    expect(pickPrimary({ day_task_ids: ['gone'] }, new Map())).toBeUndefined()
  })

  it('is stable when two strands tie, rather than flipping between renders', () => {
    const byId = new Map([
      ['a', task('a', 30)],
      ['b', task('b', 30)],
    ])
    // Strictly-greater comparison keeps the first of an equal pair.
    expect(pickPrimary({ day_task_ids: ['a', 'b'] }, byId)?.id).toBe('a')
    expect(pickPrimary({ day_task_ids: ['b', 'a'] }, byId)?.id).toBe('b')
  })
})

describe('the plan row is wired to the worklog draft', () => {
  it('opens the work via onSelect when there is work behind the ticket', () => {
    const wiring = planRowWiring()
    expect(wiring).toContain('onSelect(primary, tasks.indexOf(primary))')
  })

  it('still opens the ticket when nothing was tracked against it', () => {
    expect(planRowWiring()).toContain('onOpenTask(v.task_key, v.title)')
  })

  it('never hands a plan row an undefined onClick again', () => {
    // THE REGRESSION, exactly: `onClick={primary ? undefined : ...}` made every row
    // with work a non-interactive <div>. A row must always be given a handler now -
    // which branch it takes is the two assertions above.
    expect(planRowWiring()).not.toContain('primary ? undefined')
  })

  it('passes the draft state through, so a committed ticket can show its badge', () => {
    expect(planRowWiring()).toContain('badge={primary ? drafts?.get(primary.id) : undefined}')
  })
})

describe('PlanRow renders what it is given', () => {
  /** The `PlanRow` component body. */
  function planRow(): string {
    const at = src.indexOf('function PlanRow(')
    expect(at).toBeGreaterThan(-1)
    return src.slice(at, src.indexOf('\nfunction ', at + 1))
  }

  it('accepts a badge and renders its chip', () => {
    const body = planRow()
    expect(body).toContain('badge?: DraftBadge')
    expect(body).toContain('draftBadge(badge)')
    expect(body).toContain('{chip.label}')
  })

  it('tints only a row that is an ASK, matching OffPlanRow', () => {
    // `draftBadge` marks `drafted` and `error` loud; `posted`/`approved` are quiet
    // receipts. A done-and-filed ticket must not shout.
    expect(planRow()).toContain('const waiting = !!chip?.loud')
  })

  it('is a button whenever it has somewhere to go', () => {
    const body = planRow()
    expect(body).toContain('onClick ? (')
    expect(body).toContain('<button')
  })
})
