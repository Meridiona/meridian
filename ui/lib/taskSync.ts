//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

// One shared "pull the user's tickets in" call.
//
// # Why this exists
// The first sync after connecting a tracker is a real network round trip to Jira or
// Linear, and on a first import it is slow - tens of seconds, not a spinner's worth.
// It used to be started by `PlanView`'s backstop, which by definition cannot run
// until the planner is already on screen. So the whole cost of the sync was paid
// with the user sitting on the planner watching an empty board, being told "pulling
// your tickets in - the first sync takes a moment".
//
// Nothing about the sync needs the planner. It needs a connected tracker, and that
// exists the instant the connect completes - several seconds earlier, while the user
// is still reading the "Connected!" confirmation, picking projects, or being handed
// back by the walkthrough. Starting it there spends the wait on screens that already
// have something to look at, and the planner usually opens onto a full board.
//
// # Why it is shared state and not just an extra call
// Two callers now want the same sync: the connect flow starts it, and the planner's
// backstop still has to cover every path that does NOT come through a fresh connect
// (a reopened app, a board that emptied). Firing both would mean two outward API
// calls per connect against someone's rate limit, and a planner that reports "done"
// off the wrong one. So there is a single in-flight promise: whoever asks second
// joins the first rather than starting another.
//
// # Who calls this
// - `IntegrationConnect` - on every connect-completed path (warm start).
// - `PlanView` - the empty-board backstop and the Refresh chip (joins, or starts),
//   and `lastSyncFailure()` for the chip's hover text.

import { mutate, reportUiError } from '@/lib/bridge'

/** The sync currently running, if any. Cleared when it settles. */
let inFlight: Promise<boolean> | null = null

/** Why the last sync failed, or `null` if the last one worked (or none has run).
 *  Module state rather than a return value because the callers that need the
 *  reason and the caller that starts the sync are not the same component - the
 *  connect flow fires it, the planner's chip is what has to explain it. */
let lastFailure: string | null = null

/**
 * Render an unknown rejection as one diagnostic line.
 *
 * THE SHAPE IS NOT PREDICTABLE, and getting it wrong blanks the reason rather
 * than throwing. A `#[tauri::command]` returning `Result<_, String>` rejects with
 * a bare **string**; one returning `Result<_, SomeStruct>` rejects with the
 * serialized **object** (the blanket `Into<InvokeError>` takes that path); the
 * bridge's own guards reject with an **Error**. Reading `e.message` alone - the
 * obvious thing - yields `undefined` for two of those three.
 *
 * Never returns an empty string. An empty reason renders as "no reason recorded",
 * which is indistinguishable from the gap this whole mechanism exists to close.
 */
export function syncFailureReason(e: unknown): string {
  if (typeof e === 'string') return e.trim() || 'sync failed with an empty error'
  if (e instanceof Error) return e.message.trim() || 'sync failed with an empty Error'
  if (e && typeof e === 'object') {
    // Prefer the human field. A struct error's whole JSON body on the chip reads
    // as a crash dump - `{"code":"db.locked","detail":"pool timed out"}` where
    // "pool timed out" was the entire point - so dump the object only when it
    // carries no sentence of its own.
    for (const k of ['message', 'detail', 'error'] as const) {
      const v = (e as Record<string, unknown>)[k]
      if (typeof v === 'string' && v.trim()) return v.trim()
    }
    try {
      return JSON.stringify(e)
    } catch {
      // A cycle or a BigInt. The shape is already lost; say so rather than throw.
      return 'sync failed with an unserializable error'
    }
  }
  return 'sync failed with no error value'
}

/** Why the last sync failed, or `null` if it succeeded / none has run yet.
 *  Read by `PlanView`'s Refresh chip so hovering it says WHY, rather than the
 *  reason living only in a console a packaged build cannot open. */
export function lastSyncFailure(): string | null {
  return lastFailure
}

/**
 * Pull the latest tickets from every connected tracker.
 *
 * Joins an already-running sync instead of starting a second one, so this is safe
 * to call from anywhere that has reason to believe the board is stale.
 *
 * NEVER REJECTS, BUT DOES REPORT: resolves `true` when the sync completed and
 * `false` when it failed. The two halves of that matter for different reasons.
 *
 * Not rejecting is deliberate - most call sites (the connect flow's warm start)
 * genuinely cannot act on a failure and would only be forced into an empty
 * `.catch`. But an earlier version resolved `void`, and the one caller that DOES
 * surface a failure - the planner's Refresh chip - kept a `.catch` that could
 * therefore never fire, so "Sync failed" silently stopped appearing. A promise
 * that cannot reject has to carry its outcome in the value instead, or the
 * information is simply gone.
 *
 * # Who calls this
 * - `IntegrationConnect` - on every connect-completed path (warm start); ignores
 *   the result, because there is no sensible thing to show mid-connect.
 * - `PlanView` - the empty-board backstop and the Refresh chip (joins, or starts);
 *   the chip reads the result.
 */
export function syncTasks(): Promise<boolean> {
  if (inFlight) return inFlight
  inFlight = mutate('/api/tasks/sync', 'sync_tasks', {})
    .then(() => { lastFailure = null; return true })
    .catch((e) => {
      // A first import is a real round trip to Jira or Linear and can fail for
      // ordinary reasons (expired token, rate limit, tracker down); without a
      // reason recorded somewhere the board just stays empty and nothing explains
      // why.
      //
      // THREE PLACES, because the console alone was measurably not one of them.
      // On 2026-08-17 the chip showed `⚠ Sync failed` against a Jira sync that was
      // healthy by every other measure, and the reason could not be recovered from
      // anything: the rejection happened before the IPC dispatch, so there was no
      // Rust span, and a packaged build has no devtools to read `console.error`
      // from. The console line stays for `next dev`; `reportUiError` puts the same
      // line in the telemetry spool; `lastFailure` puts it on the chip.
      lastFailure = syncFailureReason(e)
      console.error('[meridian] tracker sync failed', e)
      reportUiError(`tracker sync failed: ${lastFailure}`)
      return false
    })
    .finally(() => { inFlight = null })
  return inFlight
}

/** The in-flight sync, or `null`. Lets a caller tell "already running" from "not
 *  started" without starting one - the planner uses it to decide whether its
 *  backstop has anything left to do. */
export function pendingTaskSync(): Promise<boolean> | null {
  return inFlight
}

/**
 * Refresh the board for a screen that is about to make a decision from the WHOLE
 * task list, and would behave differently against a stale one.
 *
 * # Only two screens should call this
 *
 * - **The daily plan**, where the user picks the day's tickets. A stale board here
 *   is not a cosmetic lag: a ticket assigned an hour ago is simply absent from the
 *   list they can pick from, so it cannot enter the plan at all. That is the
 *   candidate-starvation failure, and it is why this exists.
 * - **The retarget / match-to-existing-ticket picker**, which lists every open
 *   ticket by definition.
 *
 * Worklog drafting deliberately does NOT call this: matching reads the day's *plan*
 * as its candidate pool, not the board (`fetch_plan_candidates` - "Not the board"),
 * so a fresher board cannot widen the candidate set by even one ticket.
 *
 * # Gated, unlike `syncTasks`
 *
 * `syncTasks` FORCES a fetch, which is right for a button the user pressed and wrong
 * for a mount: a screen the user opens repeatedly would hit the tracker every time.
 * This goes through `request_gated_sync_tasks`, so the daemon applies the
 * per-provider staleness window and a re-open inside it costs one local UPSERT.
 *
 * **Never call this from a poll.** `PlanView` re-loads every 30 s and `TasksPanel`
 * every 60 s; wiring it there would rebuild the background-timer problem this whole
 * design removed, relocated into the read path. Mount and click are one-shot.
 *
 * NEVER REJECTS, for the same reason as `syncTasks`: resolves `true` on a completed
 * sync and `false` on a failure. Callers that only want fresher rows can ignore the
 * value; nothing on these screens should break because a refresh failed, since the
 * cached board still renders.
 *
 * Deliberately NOT joined to `inFlight`: that promise tracks the *forced* sync, and
 * a gated request is a different ask. It is cheap enough (one UPSERT when the window
 * is closed) that sharing state would cost more in surprise than in requests.
 */
export function requestGatedTaskSync(): Promise<boolean> {
  return mutate<{ ok: boolean }>('/api/tasks/sync', 'request_gated_sync_tasks', {})
    // `ok: false` means the daemon had not reported back inside its budget - the sync
    // is still running, so there is nothing fresher to re-read YET. Reported as
    // `false` so callers skip a pointless re-read; it is not a failure and nothing is
    // logged for it (unlike the `.catch` below).
    .then((r) => r?.ok !== false)
    .catch((e) => {
      // Reported, not surfaced: unlike the Refresh chip there is no UI element
      // waiting to explain this, and the screen renders fine from cache. It still
      // has to reach the telemetry spool, or a board that is quietly never
      // refreshing looks identical to one that is up to date.
      const reason = syncFailureReason(e)
      console.error('[meridian] gated tracker sync failed', e)
      reportUiError(`gated tracker sync failed: ${reason}`)
      return false
    })
}
