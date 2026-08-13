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
// - `PlanView` - the empty-board backstop and the Refresh chip (joins, or starts).

import { mutate } from '@/lib/bridge'

/** The sync currently running, if any. Cleared when it settles. */
let inFlight: Promise<boolean> | null = null

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
    .then(() => true)
    .catch((e) => {
      // The only record this failure leaves. A first import is a real round trip
      // to Jira or Linear and can fail for ordinary reasons (expired token, rate
      // limit, tracker down); without this the board just stays empty and there
      // is nothing anywhere to explain why.
      console.error('[meridian] tracker sync failed', e)
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
