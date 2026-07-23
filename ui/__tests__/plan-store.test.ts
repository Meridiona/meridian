//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'fs'

// Regression guard for "plan edits don't show up until 30s later".
//
// THE BUG: the daily plan has two live readers in different branches of the tree —
// the planner (PlanView, inside the Plan modal) and the timeline's "Today's focus"
// checklist (OverviewPanel, inside RightPanel). Each held the plan in its OWN
// useState and polled `get_plan` on its own 30s interval.
//
// The Plan modal is a SIBLING overlay of RightPanel, so opening/closing it never
// unmounts OverviewPanel — no remount, no refetch. Confirm/skip/save in the
// planner therefore updated only the planner's copy, and "Today's focus" showed
// stale data until its own poll happened to fire: up to 30s of lag. It was worst
// on first confirm, where the section gates on `plan.confirmed` and so stayed
// EMPTY until that delayed poll.
//
// THE FIX: one module-level store (components/plan/planStore.ts) keyed by
// calendar day. Both readers subscribe; every write publishes its returned
// PlanResponse to all of them in the same tick. `plan_action` already returned the
// fresh plan — that return value was simply being dropped.
//
// The keying is load-bearing: the plan IS per-day (daily_plan is keyed by
// plan_date), so viewing a past day must NOT react to an edit of today's plan.
//
// No React render harness in this repo (see oauth-setup-lifecycle) — we model the
// store and scan the source for the required shape.

const uiRoot = import.meta.dir + '/..'
const store = readFileSync(uiRoot + '/components/plan/planStore.ts', 'utf8')
const planView = readFileSync(uiRoot + '/components/plan/PlanView.tsx', 'utf8')
const overview = readFileSync(uiRoot + '/components/timeline/OverviewPanel.tsx', 'utf8')

// ── Model: does a write reach the other reader? ──────────────────────────────

/** The OLD shape: each reader owns a private copy, refreshed only by its poll. */
function privateStateReaders() {
  let planner: string | null = null
  let checklist: string | null = null
  return {
    write: (v: string) => { planner = v },              // only the writer's own copy
    pollChecklist: (v: string) => { checklist = v },    // 30s later…
    read: () => ({ planner, checklist }),
  }
}

/** The CURRENT shape: one keyed store, every reader a subscriber. */
function sharedStoreReaders() {
  const map = new Map<string, string>()
  const listeners = new Set<() => void>()
  const publish = (day: string, v: string) => { map.set(day, v); listeners.forEach(l => l()) }
  return {
    subscribe: () => { listeners.add(() => {}) },
    write: (day: string, v: string) => publish(day, v),
    read: (day: string) => map.get(day) ?? null,
    listenerCount: () => listeners.size,
  }
}

describe('a plan write reaches every reader immediately', () => {
  it('shared store: confirming publishes to the checklist in the same tick', () => {
    const s = sharedStoreReaders()
    s.subscribe()                        // OverviewPanel mounted, watching today
    s.write('2026-07-16', 'confirmed')   // PlanView confirms
    expect(s.read('2026-07-16')).toBe('confirmed')  // no poll needed
  })

  it('private state: the checklist stays stale until its poll — proving this discriminates', () => {
    const r = privateStateReaders()
    r.write('confirmed')
    expect(r.read().checklist).toBeNull()   // the up-to-30s window the user saw
    r.pollChecklist('confirmed')            // …only now does it catch up
    expect(r.read().checklist).toBe('confirmed')
  })

  it('is keyed by day: editing today never mutates a past day', () => {
    const s = sharedStoreReaders()
    s.write('2026-07-15', 'yesterday-plan')
    s.write('2026-07-16', 'today-plan')
    expect(s.read('2026-07-15')).toBe('yesterday-plan')
  })

  it('an unfetched day reads empty, so a day switch never shows the previous day', () => {
    const s = sharedStoreReaders()
    s.write('2026-07-16', 'today-plan')
    expect(s.read('2026-07-14')).toBeNull()
  })
})

// ── The source shape that backs the model ────────────────────────────────────

describe('planStore.ts is the single source of plan truth', () => {
  it('publishes the fresh PlanResponse a write returns', () => {
    // plan_action returns the new plan; dropping it is what forced the poll wait.
    expect(store).toMatch(/^export function planAction\(/m)
    expect(store).toMatch(/publishPlan\(date, d\)/)
  })

  it('keys entries by day and hands useSyncExternalStore a STABLE empty snapshot', () => {
    // A fresh object per getSnapshot call re-renders forever.
    expect(store).toContain('useSyncExternalStore')
    // LRU-capped (memory-leak-audit fix) rather than a bare Map — same
    // get/set semantics, bounded growth over the webview's whole lifetime.
    expect(store).toMatch(/^const store = new LruMap<string, PlanEntry>\(100\)/m)
    expect(store).toMatch(/^const EMPTY: PlanEntry = /m)
  })

  it('can hold off background refreshes during a drag', () => {
    expect(store).toMatch(/^export function pausePlanRefresh\(/m)
  })
})

describe('both readers go through the store', () => {
  it('PlanView and OverviewPanel read via usePlan', () => {
    expect(planView).toMatch(/usePlan\(todayKey\)/)
    expect(overview).toMatch(/usePlan\(day\)/)
  })

  it('neither holds the plan in local state', () => {
    expect(planView).not.toMatch(/useState<PlanResponse/)
    expect(overview).not.toMatch(/useState<PlanResponse/)
  })

  it('nothing calls get_plan / plan_action outside the store', () => {
    // A bypassing caller would update its own view and no one else's — the
    // original bug, reintroduced.
    for (const src of [planView, overview]) {
      expect(src).not.toContain("'get_plan'")
      expect(src).not.toContain("'plan_action'")
    }
  })

  it('releases the drag hold-off on unmount, so a poll can never strand paused', () => {
    // pausePlanRefresh is module-global: a planner closed mid-drag would otherwise
    // freeze every reader's refresh for the rest of the session.
    expect(planView).toMatch(/useEffect\(\(\) => \(\) => pausePlanRefresh\(false\), \[\]\)/)
  })
})
