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
import { reportUiError } from '@/lib/bridge'
import { lastSyncFailure, syncFailureReason, syncTasks } from '@/lib/taskSync'

const read = (p: string) => readFileSync(join(import.meta.dir, '..', p), 'utf8')

const TASK_SYNC = read('lib/taskSync.ts')
const PLAN_VIEW = read('components/plan/PlanView.tsx')

describe('syncTasks contract', () => {
  test('resolves an outcome rather than void, so a swallowed failure is still reportable', () => {
    expect(TASK_SYNC).toContain('export function syncTasks(): Promise<boolean>')
    expect(TASK_SYNC).toContain('return true')
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

// A SUCCESSFUL sync must never draw "Sync failed".
//
// Observed in a real run: the planner showed `⚠ Sync failed` while the daemon's
// own logs recorded the Jira fetch completing, and re-running `meridian
// tasks-sync` by hand synced 74 issues in 4s. Nothing had failed.
//
// The path: `handleSync` is `syncTasks().then(synced => load(true).then(loaded =>
// ...))` and raises the chip when EITHER is false. `load(true)` calls
// `refreshPlan`, which answers `null` when the store's drag hold-off is on - the
// same `null` a genuinely failed read returns. So a paused store made a
// successful sync report as a failure.
//
// The hold-off is module-global (`pausePlanRefresh`), so PlanView's own
// `draggingRef` guard does not cover it: another planner instance, or a drag
// whose `onDragEnd` has not run yet, holds the same flag. `initial` reads are
// user-initiated and now force past it.
describe('a paused store is not a sync failure', () => {
  const PLAN_STORE = read('components/plan/planStore.ts')

  test('refreshPlan can be forced past the drag hold-off', () => {
    expect(PLAN_STORE).toContain(
      'export function refreshPlan(date: string, force = false): Promise<PlanResponse | null>',
    )
    expect(PLAN_STORE).toContain('if (paused && !force) return Promise.resolve(null)')
  })

  test('the user-initiated read forces, so a held flag cannot read as a failure', () => {
    // `initial` is exactly the user-initiated case: the first load and every
    // `handleSync` call pass it, while the 30s background poll does not.
    expect(PLAN_VIEW).toContain('return refreshPlan(todayKey, initial).then(d => {')
    // The chip's own call must stay on that path.
    expect(PLAN_VIEW).toContain('load(true).then(loaded =>')
  })

  test('the background poll still yields to a drag', () => {
    // The hold-off must survive this fix - it is what stops a poll reshuffling
    // the board mid-drag, which is a real bug in the other direction.
    expect(PLAN_VIEW).toContain('const id = setInterval(() => load(false), 30_000)')
    expect(PLAN_STORE).toContain('export function pausePlanRefresh(next: boolean)')
  })
})

// A FAILURE WRITTEN ONLY TO THE CONSOLE IS A FAILURE NOBODY CAN READ.
//
// Measured on 2026-08-17 against 1.87.0-staging.3, after the planner drew
// `⚠ Sync failed`: `meridian tasks-sync` by hand exited 0 in 4.7s (76 Jira + 5
// GitHub issues) from both cwds `install::cli_cwd()` can resolve to,
// `pm_sync_state.last_error` was NULL for both providers, and `pm_tasks` held 81
// non-terminal board rows. Nothing about the sync had failed.
//
// What the telemetry spool held was worse than a wrong reason - it held none. The
// whole 7-day retention contained exactly three records mentioning `tasks-sync`,
// all of them `tasks-sync ok`, none from that day, and ZERO spans anywhere under
// `meridian_tray_lib::commands::tasks`. The command never ran: the invoke was
// rejected on the JS side. The only record of why was `console.error` in
// `syncTasks` - and a packaged build ships no devtools (there is no `devtools`
// feature or config anywhere in `tray/src-tauri`), while the `tray_debug`
// JS→Rust bridge is wired into the popover JS only, never the dashboard. So the
// reason went to a console with no way to open it.
//
// That specific rejection is now unrecoverable, and this does not try to guess at
// it. It pins the thing that made it unrecoverable: a sync failure must leave its
// reason somewhere a support bundle and the user can both reach.
describe('a sync failure names itself', () => {
  const BRIDGE = read('lib/bridge.ts')

  test('the reason survives whatever shape the rejection arrives in', () => {
    // A Tauri command that rejects with a String reaches JS as a bare string,
    // and one that rejects with a struct reaches it as an OBJECT (the blanket
    // Into<InvokeError> serializes it) - so `e.message` alone blanks the reason
    // for half the cases that actually occur.
    expect(syncFailureReason('tasks-sync timed out after 30s')).toBe('tasks-sync timed out after 30s')
    expect(syncFailureReason(new Error('spawn error: No such file'))).toBe('spawn error: No such file')
    // And the human field wins over the whole body: a chip reading
    // `{"code":"db.locked","detail":"pool timed out"}` is a crash dump where one
    // sentence was the entire point.
    expect(syncFailureReason({ code: 'db.locked', detail: 'pool timed out' })).toBe('pool timed out')
    expect(syncFailureReason({ message: 'expired token' })).toBe('expired token')
    // No sentence anywhere - then the body is better than nothing.
    expect(syncFailureReason({ code: 42 })).toContain('42')
    // Never empty. An empty reason reads as "no reason recorded", which is the
    // exact state this exists to end.
    expect(syncFailureReason(undefined).length).toBeGreaterThan(0)
    expect(syncFailureReason(null).length).toBeGreaterThan(0)
  })

  // THE WHOLE CHAIN, EXERCISED. A source scan cannot tell a live call from a
  // commented-out one - checked, and a `// reportUiError(...)` satisfies it - so
  // the forward has to be observed, not read.
  //
  // `bridge.ts` reaches Rust through `window.__TAURI__.core.invoke` and nothing
  // else, so standing up a fake one is enough to drive `syncTasks` end to end
  // without mocking a module. Both commands it can emit land in `sent`.
  type Sent = { cmd: string; args: unknown }
  // `unknown` cast rather than a global type augmentation: this is a test-local
  // stand-in for the real bridge, and widening the app's Window type for it would
  // make the fake look like production surface.
  const g = globalThis as unknown as { window?: unknown }
  function withFakeBridge<T>(
    invoke: (cmd: string, args: unknown) => Promise<unknown>,
    body: (sent: Sent[]) => Promise<T>,
  ): Promise<T> {
    const sent: Sent[] = []
    const original = g.window
    g.window = { __TAURI__: { core: { invoke: (cmd: string, args: unknown) => {
      sent.push({ cmd, args })
      return invoke(cmd, args)
    } } } }
    // Restored even on failure - a leaked fake `window` would silently change how
    // every later test in this process sees the bridge.
    return body(sent).finally(() => { g.window = original })
  }

  // Declared before the failure case so that one's "no reason yet" assertion holds
  // whatever order these run in: this leaves `lastFailure` null either way.
  test('a successful sync clears the previous reason', async () => {
    let broken = true
    await withFakeBridge(
      (cmd) => cmd === 'sync_tasks' && broken
        ? Promise.reject('first attempt failed')
        : Promise.resolve(null),
      async (sent) => {
        expect(await syncTasks()).toBe(false)
        expect(lastSyncFailure()).toBe('first attempt failed')
        const reportedOnce = sent.filter(s => s.cmd === 'tray_debug').length

        broken = false
        expect(await syncTasks()).toBe(true)
        // A reason left standing after a sync that worked is the same lie the chip
        // told, only in reverse - and it would be the text on the NEXT failure.
        expect(lastSyncFailure()).toBeNull()
        // Nothing to report, so nothing reported: the tray log must not fill with
        // non-events.
        expect(sent.filter(s => s.cmd === 'tray_debug').length).toBe(reportedOnce)
      },
    )
  })

  test('a rejected sync resolves false, keeps the reason, and forwards it', () =>
    // A `Result<_, String>` command rejects with a bare string - the shape
    // `tasks-sync timed out` actually arrives in.
    withFakeBridge(
      (cmd) => cmd === 'sync_tasks'
        ? Promise.reject('tasks-sync timed out after 30s')
        : Promise.resolve(null),
      async (sent) => {
        expect(lastSyncFailure()).toBeNull()
        // Never rejects - the contract above - so a caller reads the value.
        expect(await syncTasks()).toBe(false)
        expect(lastSyncFailure()).toBe('tasks-sync timed out after 30s')
        // THE POINT OF ALL OF THIS: the reason reached Rust, so it reaches the
        // telemetry spool and an Export Diagnostics bundle.
        const forwarded = sent.find(s => s.cmd === 'tray_debug')
        expect(forwarded).toBeDefined()
        expect(JSON.stringify(forwarded?.args)).toContain('tasks-sync timed out after 30s')
      },
    ))

  test('reporting a failure can never become a second failure', () => {
    // Outside Tauri this is a no-op. It runs on an error path, so a throw here
    // would replace a handled failure with an unhandled one.
    expect(() => reportUiError('probe')).not.toThrow()
    // And a rejecting `tray_debug` must not escape either.
    return withFakeBridge(() => Promise.reject(new Error('ipc down')), async () => {
      expect(() => reportUiError('probe')).not.toThrow()
    })
  })

  test('the reason is forwarded to the tray log, not just the console', () => {
    expect(BRIDGE).toContain('export function reportUiError(')
    // `tray_debug` is the command that already exists for exactly this - it logs
    // at INFO with the window label, so the line lands in the telemetry spool and
    // in an Export Diagnostics bundle.
    expect(BRIDGE).toContain("invoke('tray_debug'")
    expect(TASK_SYNC).toContain('reportUiError(')
  })

  test('the command it forwards to is still registered', () => {
    // `reportUiError` invokes `tray_debug`. Rename it, or drop it from the
    // `invoke_handler!`, and the forward silently no-ops - reintroducing the exact
    // silent-loss failure this block exists to end, one refactor later, with every
    // test still green. Cross-tree scan because the two halves live in different
    // languages and nothing else couples them.
    const TRAY_LIB = read('../tray/src-tauri/src/lib.rs')
    expect(TRAY_LIB).toContain('fn tray_debug(window: tauri::Window, msg: String)')
    // The registration line inside `generate_handler!`, not merely a mention.
    expect(TRAY_LIB).toMatch(/^\s*tray_debug,$/m)
  })

  test('the reason is readable from the chip itself', () => {
    // A screenshot of the chip is what a user sends. Hovering it must say why,
    // rather than making the reason depend on a console they cannot open.
    expect(TASK_SYNC).toContain('export function lastSyncFailure(): string | null')
    expect(PLAN_VIEW).toContain('lastSyncFailure()')
  })

  test('a null reason names the other half rather than shrugging', () => {
    // The chip is raised by `!synced || !loaded`. `lastFailure` is cleared on a
    // successful sync, so no reason means the SYNC worked and the re-read did
    // not - which is a different sentence, not a missing one. "No reason
    // recorded" here would be the same shrug this whole block exists to remove.
    expect(TASK_SYNC).toContain('lastFailure = null; return true')
    expect(PLAN_VIEW).toContain('the tickets synced but the board could not be re-read')
  })
})
