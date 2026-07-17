//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Server shapes → the one `CardTask` both plan columns render. Extracted from PlanView
// so the columns can be split out without dragging the mapping along with them.
//
// `PlanItem` (a committed row) and `AvailableTask` (a scored board candidate) carry
// almost the same fields under different names — `task_key` vs `key` — and this is the
// single place that difference is reconciled.

import type { CardTask } from '@/components/plan/TaskCard'
import type { AvailableTask, PlanItem } from '@/lib/api-types'

/** Friendly label per plan origin, for a committed row whose scored candidate we no
 *  longer have (it left the board, or the plan is being read on a past day). */
const REASON: Record<string, string> = {
  carryover: 'Carried over',
  in_progress: 'In progress',
  due_soon: 'Due soon',
  recent: 'Worked recently',
  manual: 'Added',
}

/** A scored board candidate → a card. */
export function fromAvailable(a: AvailableTask): CardTask {
  return {
    key: a.key, title: a.title, provider: a.provider, url: a.url, due_days: a.due_days,
    reason: a.reason, origin: a.origin, is_terminal: a.is_terminal,
    description: a.description, epic: a.epic, status: a.status, priority: a.priority,
    issue_type: a.issue_type, story_points: a.story_points,
  }
}

/** A committed plan row → a card. Prefers the live candidate's `reason` when the task
 *  is still on the board (it's scored against today), else falls back to a label for
 *  the stored origin. */
export function fromPlan(p: PlanItem, avail: Map<string, AvailableTask>): CardTask {
  const a = avail.get(p.task_key)
  return {
    key: p.task_key, title: p.title, provider: p.provider, url: p.url,
    due_days: p.due_days, origin: p.origin,
    reason: a?.reason ?? REASON[p.origin] ?? 'Added',
    is_terminal: p.is_terminal,
    description: p.description, epic: p.epic, status: p.status, priority: p.priority,
    issue_type: p.issue_type, story_points: p.story_points,
  }
}
