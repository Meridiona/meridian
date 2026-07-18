//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The daily plan's shared state, keyed by calendar day.
//
// WHY A MODULE-LEVEL STORE: the plan has two live readers that sit in different
// branches of the tree and never share an ancestor's state — the planner
// (PlanView, inside the Plan modal) and the timeline's "Today's focus" checklist
// (OverviewPanel, inside RightPanel). The modal is a SIBLING overlay, so opening
// and closing it never unmounts OverviewPanel — which means a confirm/skip/save
// in the planner used to leave the checklist showing stale data until its own
// 30s poll happened to fire. Both now read the same store, so a write publishes
// to every reader in the same tick.
//
// Keyed by `date` (YYYY-MM-DD) because the plan IS per-day (daily_plan is keyed
// by plan_date). That keying is load-bearing, not incidental: viewing a past day
// must NOT react to an edit of today's plan, and it doesn't — different key,
// different entry.
//
// The store holds SERVER TRUTH only. The planner's in-progress edits (its draft
// `today` list) stay local to PlanView until the user Confirms/Saves — a
// background refresh must never clobber them, which is why nothing here derives
// UI state.
//
// All I/O goes through the Tauri bridge:
//   - get_plan    → load   (flat args)  read one day's plan
//   - plan_action → mutate (body)       confirm/skip/reopen/set; returns the
//                                       fresh PlanResponse, which we publish

'use client'

import { useSyncExternalStore } from 'react'
import { load, mutate } from '@/lib/bridge'
import type { PlanResponse } from '@/lib/api-types'

/** Vestigial route label the bridge still takes (Tauri-only now). */
const API = '/api/plan'

/** One day's plan as the server last reported it. */
export interface PlanEntry {
  data: PlanResponse | null
  /** True once a read/write has resolved for this day (vs. never fetched). */
  loaded: boolean
}

// A single shared default so `getSnapshot` returns a STABLE reference for days
// not yet in the store — useSyncExternalStore re-renders forever if the snapshot
// identity changes on every call.
const EMPTY: PlanEntry = { data: null, loaded: false }

const store = new Map<string, PlanEntry>()
const listeners = new Set<() => void>()

const snapshot = (date: string): PlanEntry => store.get(date) ?? EMPTY

function emit() {
  listeners.forEach((l) => l())
}

function subscribe(l: () => void): () => void {
  listeners.add(l)
  return () => {
    listeners.delete(l)
  }
}

// Background refreshes are held off during a drag: PlanView's board is derived
// from store data, and swapping it mid-drag would shift the indices @hello-pangea/dnd
// is tracking. Only BACKGROUND reads pause — a user-initiated write still
// publishes immediately.
let paused = false

/** Hold/resume background plan refreshes (set around an active drag). */
export function pausePlanRefresh(next: boolean) {
  paused = next
}

/** Publish a server-authoritative response for `date` to every reader. */
export function publishPlan(date: string, data: PlanResponse) {
  store.set(date, { data, loaded: true })
  emit()
}

/** Re-read one day's plan and publish it. Resolves `null` when the read failed
 *  or was paused mid-drag — callers distinguish those by context (an initial
 *  load never races a drag). */
export function refreshPlan(date: string): Promise<PlanResponse | null> {
  if (paused) return Promise.resolve(null)
  return load<PlanResponse>(API, 'get_plan', { date })
    .then((d) => {
      publishPlan(date, d)
      return d
    })
    .catch(() => null)
}

/** Run a plan write for `date` and publish the fresh plan it returns, so every
 *  reader reflects it immediately rather than on its next poll. Rejects on
 *  failure (the caller surfaces the error and decides whether to roll back). */
export function planAction(
  date: string,
  action: string,
  taskKeys: string[],
): Promise<PlanResponse> {
  return mutate<PlanResponse>(API, 'plan_action', {
    action,
    task_keys: taskKeys,
    date,
  }).then((d) => {
    publishPlan(date, d)
    return d
  })
}

/** Subscribe to one day's plan. Returns a stable EMPTY entry for a day that has
 *  never been fetched. */
export function usePlan(date: string): PlanEntry {
  return useSyncExternalStore(
    subscribe,
    () => snapshot(date),
    () => EMPTY,
  )
}
