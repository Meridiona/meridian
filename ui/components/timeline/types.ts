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
  proposal: 'var(--color-state-proposal)',
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

/** Kind label shown next to the ticket key ("Bug" / "Task" / "Story"), prefixed
 *  "New " for a live proposal. Falls back to a generic "Work log" when the row
 *  carries no issue type. */
export function kindLabel(w: WorklogItem): string {
  const kind = (w.issue_type ?? '').trim() || 'Work log'
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
