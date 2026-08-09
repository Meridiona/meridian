//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The worklog state machine for one day-task, extracted from DayTaskDetailPanel
// so the panel stays presentational and this — the get/generate/approve flow and
// its every-provider error handling — lives in one testable place.
//
// STATE LIVES IN A MODULE-LEVEL STORE, not in the hook. `generate` can take ~a
// minute (an LLM call in the Rust CLI); the detail panel unmounts the moment you
// select another card, which would destroy component-local state and orphan the
// in-flight request. Keying the state by `(day, taskId)` in a store that outlives
// the panel means switching away and back preserves the "generating…" state and
// picks up the result when it lands. The hook is a thin `useSyncExternalStore`
// view over that store.
//
// All I/O goes through the Tauri bridge:
//   - get      → load  (flat args)   read, returns null when there's no draft
//   - generate → invoke (flat args)  the LLM call (can take ~a minute)
//   - approve  → mutate (body)       create + post, idempotent server-side
// Flat args (get/generate) cross the bridge as camelCase (`taskId`); the approve
// body struct keeps snake_case (`task_id`) — see worklog_generate.rs.

'use client'

import { useEffect, useState, useSyncExternalStore } from 'react'
import { load, invoke, mutate } from '@/lib/bridge'
import type { DayTaskWorklogDraft, ApproveWorklogResponse } from '@/lib/api-types'
import { LruMap } from '@/lib/lru'

/** Where the flow is right now — drives which footer control the panel shows. */
export type WorklogPhase = 'loading' | 'idle' | 'generating' | 'approving'

export interface WorklogState {
  draft: DayTaskWorklogDraft | null
  phase: WorklogPhase
  error: string | null
  /** True once the update has been posted — Generate/Approve are then disabled. */
  posted: boolean
  /** The user is confirming the post (Approve tapped, awaiting "Yes, post"). */
  confirming: boolean
  setConfirming: (v: boolean) => void
  /** Run (or regenerate, overwriting) the AI draft. */
  generate: () => void
  /** Approve the current draft: create-if-proposed, post the comment to every
   *  target, link it. Safe to retry after a partial post - already-posted tickets
   *  are skipped server-side. */
  approve: () => void
  /** Point the draft at ONE ticket the user picked over the whole board, dropping
   *  the AI's. The AI only ever matches against the day's planned tasks, so this is
   *  the override for unplanned work. No LLM call - it retargets, it doesn't
   *  rewrite. */
  retarget: (taskKey: string) => void
  /** Drop one ticket the AI matched, keeping the rest. */
  dismiss: (taskKey: string) => void
  /** Choose which connected tracker a PROPOSED new ticket is created on. A no-op
   *  on a matched draft, where each target carries its own provider. */
  setProvider: (provider: string) => void
  /** Re-read the draft, discarding the cached copy.
   *
   *  For writes that happen OUTSIDE this machine and change what the draft points
   *  at. Escalating a personal task is the one that exists: it repoints
   *  `day_task_worklog_targets` at the real ticket in the daemon, so without this
   *  the footer would keep naming the local key that no longer exists. */
  refresh: () => void
}

const errMsg = (e: unknown): string =>
  e instanceof Error ? e.message : typeof e === 'string' ? e : 'Something went wrong'

const API = '/api/day-task-worklog' // vestigial route label the bridge wants (Tauri-only now)

// ── The external store (module-level, survives panel unmount) ─────────────────

interface Entry {
  draft: DayTaskWorklogDraft | null
  phase: WorklogPhase
  error: string | null
  /** True once the initial get has resolved for this key (don't re-load). */
  loaded: boolean
}

// A single shared default so `getSnapshot` returns a STABLE reference for keys not
// yet in the store (useSyncExternalStore loops forever if the snapshot identity
// changes every call).
const EMPTY: Entry = { draft: null, phase: 'loading', error: null, loaded: false }

// LRU-capped: this webview session never reloads, so an uncapped Map would
// grow by one entry per distinct (day, taskId) pair ever opened for the
// app's whole lifetime. 100 is far beyond realistic usage — a hygiene bound,
// not a fix for an observed runaway. (An entry mid `generating`/`approving`
// stays the most-recently-touched one via the `patch`→`store.set` on every
// state change, so it's never the eviction candidate while active.)
const store = new LruMap<string, Entry>(100)
const listeners = new Set<() => void>()

// A space is a safe separator here: `day` is YYYY-MM-DD and `taskId` is T1/T2…,
// so neither half can contain one and no two cards can collide on a key.
const keyOf = (day: string, taskId: string) => `${day} ${taskId}`
const snapshot = (key: string): Entry => store.get(key) ?? EMPTY

function patch(key: string, next: Partial<Entry>) {
  store.set(key, { ...snapshot(key), ...next })
  listeners.forEach((l) => l())
}

function subscribe(l: () => void): () => void {
  listeners.add(l)
  return () => listeners.delete(l)
}

/** Load the existing draft once per key. No-op if it's already loaded or a
 *  generate/approve is in flight (never clobber live state with a stale read). */
function ensureLoaded(day: string, taskId: string) {
  const key = keyOf(day, taskId)
  const e = store.get(key)
  if (e && (e.loaded || e.phase === 'generating' || e.phase === 'approving')) return
  patch(key, { phase: 'loading', error: null })
  load<DayTaskWorklogDraft | null>(API, 'get_day_task_worklog', { day, taskId })
    .then((r) => {
      // A generate/approve may have started while the read was in flight — don't
      // stomp it.
      const cur = store.get(key)
      if (cur && (cur.phase === 'generating' || cur.phase === 'approving')) return
      patch(key, { draft: r ?? null, phase: 'idle', loaded: true })
    })
    .catch(() => patch(key, { phase: 'idle', loaded: true }))
}

/** Run (or regenerate) the AI draft for `(day, taskId)`. Idempotent against a
 *  double-tap: a second call while one is running is ignored. */
function runGenerate(day: string, taskId: string) {
  const key = keyOf(day, taskId)
  if (store.get(key)?.phase === 'generating') return
  patch(key, { phase: 'generating', error: null })
  invoke<DayTaskWorklogDraft>('generate_day_task_worklog', { day, taskId })
    .then((r) => patch(key, { draft: r, phase: 'idle', loaded: true, error: r.error ?? null }))
    .catch((e) => patch(key, { phase: 'idle', error: errMsg(e) }))
}

/** Approve the current draft: create-if-proposed then post to every target.
 *
 *  Re-reads the draft rather than patching the local copy from the response. An
 *  approve can land on some tickets and fail on others, so the resulting state is
 *  per-ticket (which posted, which carry an error, whether the row went `posted` at
 *  all) — and the server already computed all of it. Reconstructing that here from
 *  the response would be a second implementation of the same rules, free to drift.
 *
 *  Idempotent server-side (an already-posted ticket is never posted to twice) and
 *  against a double-tap here.
 */
function runApprove(day: string, taskId: string) {
  const key = keyOf(day, taskId)
  if (store.get(key)?.phase === 'approving') return
  patch(key, { phase: 'approving', error: null })
  mutate<ApproveWorklogResponse>(API, 'approve_day_task_worklog', { day, task_id: taskId })
    .then(async (r) => {
      const fresh = await load<DayTaskWorklogDraft | null>(API, 'get_day_task_worklog', {
        day,
        taskId,
      }).catch(() => null)
      const failed = r.targets.filter((t) => !t.posted)
      const error =
        r.error ||
        (failed.length
          ? `Could not post to ${failed.map((t) => t.task_key).join(', ')} - the rest went through. Try again to finish.`
          : null)
      patch(key, { phase: 'idle', loaded: true, error, ...(fresh ? { draft: fresh } : {}) })
    })
    .catch((e) => patch(key, { phase: 'idle', error: errMsg(e) }))
}

/** Drop one ticket from the draft's target set. Like retarget, a plain DB write. */
function runDismiss(day: string, taskId: string, taskKey: string) {
  const key = keyOf(day, taskId)
  const cur = store.get(key)
  if (cur?.phase === 'approving' || cur?.phase === 'generating') return
  patch(key, { phase: 'approving', error: null })
  mutate<DayTaskWorklogDraft>(API, 'dismiss_worklog_target', {
    day,
    task_id: taskId,
    task_key: taskKey,
  })
    .then((r) => patch(key, { draft: r, phase: 'idle', loaded: true, error: r.error ?? null }))
    .catch((e) => patch(key, { phase: 'idle', error: errMsg(e) }))
}

/** Choose which tracker a PROPOSED new ticket gets created on.
 *
 *  Only meaningful on the propose branch: a matched draft's provider is per-ticket
 *  and the command refuses there. Like retarget/dismiss it's a plain DB write, so it
 *  reuses the `approving` phase - "a write is in flight, don't touch the draft" - and
 *  renders from the updated draft the server returns. */
function runSetProvider(day: string, taskId: string, provider: string) {
  const key = keyOf(day, taskId)
  const cur = store.get(key)
  if (cur?.phase === 'approving' || cur?.phase === 'generating') return
  // Already there — skipping the round-trip keeps a tap on the selected chip from
  // flickering the panel through a write state for no change.
  if (cur?.draft?.provider === provider) return
  patch(key, { phase: 'approving', error: null })
  mutate<DayTaskWorklogDraft>(API, 'set_worklog_provider', {
    day,
    task_id: taskId,
    provider,
  })
    .then((r) => patch(key, { draft: r, phase: 'idle', loaded: true, error: r.error ?? null }))
    .catch((e) => patch(key, { phase: 'idle', error: errMsg(e) }))
}

/** Retarget the draft at a user-picked ticket. Unlike generate this is a plain DB
 *  write (no LLM, no CLI spawn), so it resolves in milliseconds — but it still
 *  goes through the store, because the panel can unmount mid-flight like anything
 *  else here. Reuses the `approving` phase: both mean "a write is in flight, don't
 *  touch the draft", and a third phase would have to be handled at every branch
 *  in the footer for no behavioural difference. */
function runRetarget(day: string, taskId: string, taskKey: string) {
  const key = keyOf(day, taskId)
  const cur = store.get(key)
  if (cur?.phase === 'approving' || cur?.phase === 'generating') return
  patch(key, { phase: 'approving', error: null })
  mutate<DayTaskWorklogDraft>(API, 'retarget_day_task_worklog', {
    day,
    task_id: taskId,
    task_key: taskKey,
  })
    .then((r) => patch(key, { draft: r, phase: 'idle', loaded: true, error: r.error ?? null }))
    .catch((e) => patch(key, { phase: 'idle', error: errMsg(e) }))
}

/** Force a re-read of the draft, ignoring `loaded`.
 *
 *  `ensureLoaded` deliberately reads once per key and never again, which is right
 *  for a draft only this machine writes. Escalation is the exception - the daemon
 *  repoints the draft's targets at the real ticket - so this is the way to pick
 *  that up without inventing what the new state is from the escalate response.
 *  Still refuses to stomp an in-flight generate/approve, same rule as everywhere
 *  else here. */
function runRefresh(day: string, taskId: string) {
  const key = keyOf(day, taskId)
  const cur = store.get(key)
  if (cur?.phase === 'generating' || cur?.phase === 'approving') return
  load<DayTaskWorklogDraft | null>(API, 'get_day_task_worklog', { day, taskId })
    .then((r) => {
      const now = store.get(key)
      if (now && (now.phase === 'generating' || now.phase === 'approving')) return
      patch(key, { draft: r ?? null, phase: 'idle', loaded: true })
    })
    .catch(() => {})
}

// ── The hook: a thin view over the store ──────────────────────────────────────

/** Own the worklog flow for `(day, taskId)`. Reads live state from the module
 *  store so it survives the panel unmounting mid-generation. */
export function useWorklog(day: string, taskId: string): WorklogState {
  const key = keyOf(day, taskId)
  const entry = useSyncExternalStore(
    subscribe,
    () => snapshot(key),
    () => EMPTY,
  )
  // `confirming` is a transient per-view toggle (Approve tapped → awaiting "Yes,
  // post"); intentionally local, and reset when the selected task changes so you
  // never return to a card mid-confirm.
  const [confirming, setConfirming] = useState(false)

  useEffect(() => {
    setConfirming(false)
    ensureLoaded(day, taskId)
  }, [day, taskId])

  return {
    draft: entry.draft,
    phase: entry.phase,
    error: entry.error,
    posted: entry.draft?.state === 'posted',
    confirming,
    setConfirming,
    generate: () => {
      setConfirming(false)
      runGenerate(day, taskId)
    },
    approve: () => {
      setConfirming(false)
      runApprove(day, taskId)
    },
    retarget: (taskKey: string) => {
      setConfirming(false)
      runRetarget(day, taskId, taskKey)
    },
    dismiss: (taskKey: string) => {
      setConfirming(false)
      runDismiss(day, taskId, taskKey)
    },
    // Changing where a proposed ticket lands invalidates a pending confirm: the
    // user is now agreeing to create it on a different board than the one the
    // confirm prompt named.
    setProvider: (provider: string) => {
      setConfirming(false)
      runSetProvider(day, taskId, provider)
    },
    refresh: () => runRefresh(day, taskId),
  }
}
