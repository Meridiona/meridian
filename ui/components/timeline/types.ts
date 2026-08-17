//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Shared types, formatting helpers, and small pure predicates for the worklog
// timeline. Split out of the view so `useWorklogsForDay`, the grid, the detail
// pane, and the review overlay all read the same vocabulary instead of each
// re-deriving it.

import type { WorklogItem } from '@/lib/api-types'

// Where the reviewer says the time should have gone, supplied on reject.
// Empty = plain dismissal. correctedToUntracked wins if both are set server-side.
export type RejectCorrection = { correctedTaskKey?: string; correctedToUntracked?: boolean }

export type Candidate = { key: string; title: string }

// The four theme-independent semantic states a card's accent bar / status chip
// is keyed on (mock's card anatomy). Merged from the old per-`state` STATE_STYLE
// but collapsed onto the fixed --color-state-* tokens defined in globals.css.
export type VisualState = 'approved' | 'rejected' | 'proposal' | 'pending'

export const VISUAL_STATE_COLOR: Record<VisualState, string> = {
  approved: 'var(--color-state-approved)',
  rejected: 'var(--color-state-rejected)',
  proposal: 'var(--t-accent)',
  pending: 'var(--color-state-pending)',
}

const STATE_LABEL: Record<string, string> = {
  drafted: 'Draft',
  proposed: 'Proposed',
  approved: 'Approved',
  posted: 'Posted',
  skipped: 'Dismissed',
  dismissed: 'Dismissed',
  failed: 'Failed',
}

/** Collapse a worklog's raw `state` (+ proposed-ness) into the visual state that
 *  drives its accent bar and status-pill color. A proposed row awaiting the
 *  daemon sweep (`approved`) reads as approved (green), not a live proposal. */
export function visualState(w: WorklogItem): VisualState {
  if (w.is_proposed && w.state === 'proposed') return 'proposal'
  if (w.state === 'skipped' || w.state === 'dismissed' || w.state === 'failed') return 'rejected'
  if (w.state === 'approved' || w.state === 'posted') return 'approved'
  return 'pending'
}

/** Accent-bar / chip color for a worklog, via its visual state. */
export function stateColor(w: WorklogItem): string {
  return VISUAL_STATE_COLOR[visualState(w)]
}

/** Uppercase status-chip label for a worklog. */
export function stateLabel(w: WorklogItem): string {
  return STATE_LABEL[w.state] ?? w.state
}

/** Human-readable reason a `failed` worklog didn't post, for known
 *  unrecoverable provider errors — the raw message (`last_post_error`) is
 *  API-response text (e.g. Jira's `{"errorMessages":["WORKLOGS_PER_ISSUE_LIMIT_EXCEEDED: 10000"]}`),
 *  not something to show a user as-is. Falls back to the raw message for
 *  anything not specifically recognized, so nothing is ever hidden — just
 *  clarified where possible. Always points at the fix (re-match via Edit)
 *  since that's the one recovery path every `failed` row has. */
export function failureReason(error: string): string {
  if (error.includes('WORKLOGS_PER_ISSUE_LIMIT_EXCEEDED')) {
    return "This ticket has hit Jira's 10,000-worklog limit and can never accept another entry. Click Edit to re-match this worklog to a different ticket, then approve it again."
  }
  return error
}

/** Kind label shown next to the ticket key — the ticket's actual issue type
 *  ("Bug" / "Task" / "Story"), prefixed "New " for a live proposal. Never
 *  "Work log" (that's the card's own generic content, not the ticket's type):
 *  falls back to "Task" only when the row carries no issue type at all (its
 *  `pm_tasks` row is missing or was never fetched from the tracker). */
export function kindLabel(w: WorklogItem): string {
  const kind = (w.issue_type ?? '').trim() || 'Task'
  return w.is_proposed && w.state === 'proposed' ? `New ${kind}` : kind
}

// Local YYYY-MM-DD for `d` days from today (negative = past).
export function dayString(offsetDays = 0): string {
  const d = new Date()
  d.setDate(d.getDate() + offsetDays)
  const y = d.getFullYear()
  const m = String(d.getMonth() + 1).padStart(2, '0')
  const day = String(d.getDate()).padStart(2, '0')
  return `${y}-${m}-${day}`
}

export function shiftDay(d: string, by: number): string {
  const dt = new Date(`${d}T12:00:00`)
  dt.setDate(dt.getDate() + by)
  const today = new Date(); today.setHours(12, 0, 0, 0)
  if (dt > today) return d // never go past today
  const y = dt.getFullYear(); const m = String(dt.getMonth() + 1).padStart(2, '0'); const day = String(dt.getDate()).padStart(2, '0')
  return `${y}-${m}-${day}`
}

// Toolbar date-nav label for a non-today day — "Tue, Jun 30", not the raw
// YYYY-MM-DD. `isToday` (in the toolbar) covers the "Today" case separately.
export function formatDayLabel(d: string): string {
  return new Date(`${d}T12:00:00`).toLocaleDateString('en-US', {
    weekday: 'short', month: 'short', day: 'numeric',
  })
}

// Human label for a worklog's tracker (provider snapshot on the row).
export function providerLabel(provider: string): string {
  switch (provider) {
    case 'jira': return 'Jira'
    case 'linear': return 'Linear'
    case 'github': return 'GitHub'
    default: return provider || 'Jira'
  }
}

// Namespaced busy/selection key: worklogs and proposed tasks share an
// autoincrement id sequence across two tables and can collide (pm_worklogs.id=5
// and pm_proposed_tasks.id=5 can coexist), so every id used as a React key or a
// busy-lock key must go through this.
export function itemKey(w: Pick<WorklogItem, 'id' | 'is_proposed'>): string {
  return w.is_proposed ? `prop:${w.id}` : `wl:${w.id}`
}

// A proposed item is pending only while still `state === 'proposed'`. Once
// approved it carries its real state (`approved`) and stays visible on the
// timeline — awaiting the daemon's proposal sweep to create the real ticket —
// without needing further review, so it's no longer "pending". Dismissed
// proposals and created-ticket rows never come back from `get_worklogs` at
// all (see meridian-core/src/readers/worklogs.rs::append_proposed_items).
// Real worklogs are pending only while drafted.
export function isPending(w: WorklogItem): boolean {
  return w.is_proposed ? w.state === 'proposed' : w.state === 'drafted'
}

// Should OverviewPanel's "Today's focus" section render at all?
//
// Deliberately takes NO `isSolo` argument. It used to: the populated checklist
// and a past day's read-only note were gated on `!isSolo`, while the
// empty-today nudge was shown to everyone. That combination is incoherent — a
// solo user was invited to plan (the nudge), PlanView's composer let them
// commit a personal, tracker-free task, and then the committed plan rendered
// nowhere, because the moment it stopped being empty the only branch that
// admitted solo users stopped matching. A confirmed plan disappearing the
// instant you confirm it is the bug this predicate exists to prevent
// recurring; a tracker is not what makes a plan worth showing you.
//
// `planLoaded` is whether `get_plan` has resolved for this day. It gates the
// empty case only, so an empty section never flashes before the first fetch —
// items already imply a resolved fetch.
/** Which of a day's plan rows "Today's focus" should show.
 *
 *  ROWS IN THE PLAN MEAN A USER PUT THEM THERE. Suggestions live in
 *  `plan.suggestions` / `plan.available`, never in `plan.plan`, so a non-empty
 *  `plan` is by construction a committed one. This used to read
 *  `plan.confirmed ? plan.plan : []`, and that gate has now hidden real tasks
 *  twice through two different write paths:
 *
 *    1. the composer's `add`, which wrote rows without stamping
 *       `daily_plan_meta` at all (fixed in the Rust `add` arm);
 *    2. `reopen`, which CLEARED the stamp while leaving the rows - so "Skip
 *       today" then "Plan today →" left five tasks on screen in the planner and
 *       the "What are you working on today?" nudge on the dashboard.
 *
 *  Both were write-side bugs and both are fixed there. Reading the rows rather
 *  than the flag means a third one cannot blank this screen: the worst a missing
 *  stamp can now do is affect surfaces that genuinely need to know whether the
 *  ritual was performed, not hide work the user can see elsewhere in the app.
 *
 *  `skipped` IS still honoured, and this is where the old gate was wrong in the
 *  other direction: skipping stamps `confirmed_at`, so `confirmed` was true on a
 *  skipped day and leftover rows rendered under a day the user had explicitly
 *  declined to plan. Matching Rust's [`DayPlan::is_planned`] - not skipped, and
 *  actually holding something - puts the dashboard and the daily summary back on
 *  one rule. */
export function visibleFocusItems<T>(
  plan: { plan: T[]; skipped: boolean } | null | undefined,
): T[] {
  if (!plan || plan.skipped) return []
  return plan.plan
}

export function focusSectionVisible({ isToday, planLoaded, itemCount }: {
  isToday: boolean
  planLoaded: boolean
  itemCount: number
}): boolean {
  // A past date always renders: that day's committed focus, or a quiet
  // "nothing planned" note. Editing affordances are suppressed separately.
  if (!isToday) return true
  // Today with a committed plan → the checklist.
  if (itemCount > 0) return true
  // Today with nothing planned → the nudge, once the fetch has resolved.
  return planLoaded
}
