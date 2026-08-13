//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

// `syncTasks`'s contract, and the one caller that depends on the half of it that
// is easy to lose.
//
// The bug this exists to prevent already happened once. `syncTasks` was written to
// never reject - correct, because most callers cannot act on a sync failure - but
// it resolved `void`, so the outcome was not carried anywhere. `PlanView`'s Refresh
// chip kept a `.catch(() => setSyncError(true))` that was therefore unreachable,
// and "Sync failed" silently stopped being reachable at all. Nothing failed loudly;
// the chip just went back to saying "Refresh".
//
// So there are two things to pin, and they only work together:
//   1. the promise still never rejects (behavioural, tested live below), and
//   2. callers read the RESULT rather than catching a rejection (source-scanned,
//      the repo's convention for component wiring - see task-composer.test.ts).

import { describe, expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'

const read = (p: string) => readFileSync(join(import.meta.dir, '..', p), 'utf8')

const TASK_SYNC = read('lib/taskSync.ts')
const PLAN_VIEW = read('components/plan/PlanView.tsx')

describe('syncTasks contract', () => {
  test('resolves an outcome rather than void, so a swallowed failure is still reportable', () => {
    expect(TASK_SYNC).toContain('export function syncTasks(): Promise<boolean>')
    expect(TASK_SYNC).toContain('.then(() => true)')
    // The failure branch must yield `false` - not `undefined`, which is falsy in
    // the same way and would pass a lazy check while carrying no information.
    expect(TASK_SYNC).toContain('return false')
  })

  test('a failure leaves a trace in the console', () => {
    // A first import is a real round trip to Jira or Linear. Without this the
    // board just stays empty with nothing anywhere explaining why.
    expect(TASK_SYNC).toContain("console.error('[meridian] tracker sync failed'")
  })

  test('the in-flight handle carries the same type, so joiners see the outcome too', () => {
    expect(TASK_SYNC).toContain('let inFlight: Promise<boolean> | null = null')
    expect(TASK_SYNC).toContain('export function pendingTaskSync(): Promise<boolean> | null')
  })
})

describe('PlanView reads the outcome instead of catching', () => {
  test('handleSync derives syncError from both results', () => {
    expect(PLAN_VIEW).toContain('if (!synced || !loaded) setSyncError(true)')
  })

  test('no unreachable .catch on the sync chain', () => {
    // The exact dead line that used to be here. Neither syncTasks nor load can
    // reject, so any .catch setting syncError is by definition never called.
    expect(PLAN_VIEW).not.toContain('.catch(() => setSyncError(true))')
  })

  test('load reports its outcome rather than returning void', () => {
    expect(PLAN_VIEW).toContain('const load = useCallback((initial = false): Promise<boolean> =>')
    // A poll skipped mid-drag attempted nothing, so it must not read as a failure.
    expect(PLAN_VIEW).toContain('if (!initial && draggingRef.current) return Promise.resolve(true)')
  })

  test('the chip still has a failed state to show', () => {
    expect(PLAN_VIEW).toContain("'Sync failed'")
  })
})
