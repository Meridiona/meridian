//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// The morning planner's shell. It owns four things and delegates everything else:
//   - the day key + the shared plan store subscription,
//   - the DnD context and `onDragEnd` (which moves cards BETWEEN the two columns, so
//     the `today` list can't live in either one of them),
//   - the mode algebra (proposed / editing / confirmed / skipped),
//   - the task-detail dialog.
//
// The columns themselves are PlanTodayColumn / PlanBoardColumn, and authoring a task
// is TaskComposer. See planStore.ts for why server truth lives outside this component.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { DragDropContext, type DropResult } from '@hello-pangea/dnd'
import type { CardTask } from '@/components/plan/TaskCard'
import { PlanTodayColumn } from '@/components/plan/PlanTodayColumn'
import { PlanBoardColumn } from '@/components/plan/PlanBoardColumn'
import { TaskComposer } from '@/components/plan/TaskComposer'
import { fromAvailable, fromPlan } from '@/components/plan/planNormalise'
import { PRIMARY_BTN } from '@/components/plan/planStyles'
import { TaskDetailDialog } from '@/components/timeline/TaskDetailDialog'
import { dayString } from '@/components/timeline/types'
import type { PlanResponse, IntegrationsResponse } from '@/lib/api-types'
import { MAX_PLAN_TASKS } from '@/lib/api-types'
import { load as bridgeLoad } from '@/lib/bridge'
import { connectedTrackers } from '@/lib/integrations'
import { usePlan, refreshPlan, planAction, pausePlanRefresh } from '@/components/plan/planStore'

export default function PlanView() {
  // The planner always edits TODAY (the timeline gates "Edit plan" to today), so
  // it keys the shared store by today's local day — the same key OverviewPanel
  // uses while viewing today, which is what makes a confirm/save here land in
  // its "Today's focus" instantly. Fixed at mount: the planner is a modal, so a
  // midnight rollover re-keys on the next open.
  const todayKey = useMemo(() => dayString(0), [])
  // Server truth lives in the shared store; the draft below (`today`) stays local
  // until the dev Confirms/Saves.
  const { data } = usePlan(todayKey)
  const [loadFailed, setLoadFailed] = useState(false)
  const [today, setToday] = useState<CardTask[]>([])
  const [confirmedMode, setConfirmedMode] = useState(false)
  const [editing, setEditing] = useState(false)
  const [skipped, setSkipped] = useState(false)
  const [search, setSearch] = useState('')
  const [sortMode, setSortMode] = useState<'top' | 'due' | 'az'>('top')
  const [saveError, setSaveError] = useState(false)
  // An add was refused for hitting the plan cap. Cleared by the next successful
  // commit, so it never lingers past the thing it describes.
  const [capHit, setCapHit] = useState(false)
  const [openTask, setOpenTask] = useState<CardTask | null>(null)
  const [trackers, setTrackers] = useState<ReturnType<typeof connectedTrackers>>([])
  const draggingRef = useRef(false)

  const derive = useCallback((d: PlanResponse) => {
    const isConfirmed = d.confirmed && !d.skipped
    setConfirmedMode(isConfirmed)
    setEditing(false)   // re-deriving from the server always returns to the locked/clean state
    setSkipped(d.skipped)
    const avail = new Map(d.available.map(a => [a.key, a]))
    if (isConfirmed || d.plan.length > 0) setToday(d.plan.map(p => fromPlan(p, avail)))
    else if (!d.skipped) setToday(d.suggestions.map(fromAvailable))
    else setToday([])
  }, [])

  // `initial` (mount / error-rollback) re-seeds the Today list from the server.
  // A background poll passes initial=false: it refreshes the board (so a PM sync's
  // new/changed tickets appear) WITHOUT re-deriving Today, so it never clobbers the
  // user's in-progress edits. Skipped entirely during an active drag — the store
  // holds off background refreshes for the drag's duration (see pausePlanRefresh),
  // which also stops OTHER readers' polls from swapping the board mid-drag.
  const load = useCallback((initial = false) => {
    if (!initial && draggingRef.current) return
    // A thrown error = real backend failure (not an empty day) → surface, don't
    // render empty. `refreshPlan` publishes to the store, so `data` arrives via
    // usePlan rather than local state.
    refreshPlan(todayKey).then(d => {
      if (!initial) return
      if (d) { setLoadFailed(false); derive(d) } else setLoadFailed(true)
    })
  }, [derive, todayKey])

  useEffect(() => {
    load(true)
    const id = setInterval(() => load(false), 30_000)
    return () => clearInterval(id)
  }, [load])

  // Which trackers are connected decides whether the composer offers a "file it on
  // my board" choice at all. A failure here is not fatal: [] means personal-only,
  // which is the safe default (it never files anything outward).
  useEffect(() => {
    bridgeLoad<IntegrationsResponse>('/api/integrations', 'get_integrations')
      .then(r => setTrackers(connectedTrackers(r)))
      .catch(() => setTrackers([]))
  }, [])

  // The drag hold-off is module-global, so a planner closed mid-drag (onDragEnd
  // never fires) would strand every reader's refresh paused for the rest of the
  // session. Always release it on unmount.
  useEffect(() => () => pausePlanRefresh(false), [])

  // Persist a Today ordering (live mode only); roll back to server truth on error.
  // The write publishes the fresh plan to the store, so the timeline's "Today's
  // focus" reflects a Save immediately instead of on its next poll.
  const persist = useCallback((keys: string[]) => {
    planAction(todayKey, 'set', keys)
      .then(() => setSaveError(false))
      .catch(() => { setSaveError(true); load(true) })   // rollback to server truth
  }, [load, todayKey])

  const metaAction = useCallback((action: string, keys: string[]) => {
    planAction(todayKey, action, keys)
      .then(d => { setSaveError(false); derive(d) })
      .catch(() => setSaveError(true))
  }, [derive, todayKey])

  // Local edit only — both the proposed draft and the "Edit plan" session keep
  // changes in `today` until the dev explicitly Confirms / Saves. No silent writes.
  //
  // THE CHOKE POINT for the plan cap. Every add — drag from the board, the board
  // card's + button, the detail dialog's Add, a fresh create — lands here, so the
  // guard lives here rather than at four call sites that would drift apart.
  //
  // It refuses a GROWING list only. An over-cap plan that already exists (seeded
  // before the cap, or from another machine) must stay editable, and refusing on
  // length alone would freeze it: you couldn't even remove a task to get back
  // under. Rust enforces the same rule on the way out (plan::check_plan_size);
  // this just says so a second earlier, without a round trip.
  const commit = useCallback((next: CardTask[]) => {
    if (next.length > today.length && next.length > MAX_PLAN_TASKS) {
      setCapHit(true)
      return
    }
    setCapHit(false)
    setToday(next)
  }, [today.length])

  // editable = the dev can drag / add / remove right now: either the pre-confirm
  // draft, or an unlocked "Edit plan" session. A confirmed-but-locked plan is read-only.
  const proposed = !confirmedMode && !skipped
  const editable = proposed || editing

  const saveEdits = useCallback(() => {
    persist(today.map(t => t.key))
    setEditing(false)
  }, [persist, today])

  const cancelEdits = useCallback(() => {
    setEditing(false)
    load(true)   // discard local changes, restore the committed plan from the server
  }, [load])

  // A create already added the task to the plan server-side, so re-derive from the
  // server rather than guessing the row — that's also what picks up its real key
  // when a tracker create synced one back.
  const onCreated = useCallback(() => load(true), [load])

  // ── derived board + key map ─────────────────────────────────────────────────
  const byKey = useMemo(() => {
    const m = new Map<string, CardTask>()
    ;(data?.available ?? []).forEach(a => m.set(a.key, fromAvailable(a)))
    today.forEach(t => m.set(t.key, t))
    return m
  }, [data, today])

  const board = useMemo(() => {
    const todayKeys = new Set(today.map(t => t.key))
    let items = (data?.available ?? []).filter(a => !todayKeys.has(a.key))
    const q = search.trim().toLowerCase()
    if (q) items = items.filter(a => a.key.toLowerCase().includes(q) || a.title.toLowerCase().includes(q))
    const sorted = [...items]
    if (sortMode === 'due') sorted.sort((a, b) => (a.due_days ?? 9e9) - (b.due_days ?? 9e9) || b.score - a.score || a.key.localeCompare(b.key))
    else if (sortMode === 'az') sorted.sort((a, b) => a.title.localeCompare(b.title) || a.key.localeCompare(b.key))
    return sorted.map(fromAvailable)
  }, [data, today, search, sortMode])

  // ── single onDragEnd for both lists (@hello-pangea/dnd DropResult) ──────────
  const onDragEnd = useCallback((result: DropResult) => {
    draggingRef.current = false
    pausePlanRefresh(false)
    if (!editable) return                                      // plan is locked — ignore stray drags
    const { source, destination, draggableId } = result
    if (!destination) return                                   // dropped outside any list
    const from = source.droppableId
    const to = destination.droppableId

    if (from === 'today' && to === 'today') {                  // reorder within Today
      if (source.index === destination.index) return
      const next = [...today]
      const [moved] = next.splice(source.index, 1)
      next.splice(destination.index, 0, moved)
      commit(next)
      return
    }
    if (from === 'board' && to === 'today') {                  // add at the drop position
      const task = byKey.get(draggableId)
      if (!task || today.some(t => t.key === draggableId)) return
      const next = [...today]
      next.splice(destination.index, 0, task)
      commit(next)
      return
    }
    if (from === 'today' && to === 'board') {                  // drag out → remove
      commit(today.filter(t => t.key !== draggableId))
      return
    }
    // board → board: it's a sorted source list, leave order to the sort control.
  }, [today, byKey, commit, editable])

  // ── render ──────────────────────────────────────────────────────────────────
  if (!data) {
    return (
      <div className="h-full flex flex-col p-6">
        {loadFailed
          ? <button onClick={() => load(true)} className={PRIMARY_BTN} style={{ background: 'var(--color-state-approved)', color: '#fff', alignSelf: 'flex-start' }}>Couldn’t load — retry</button>
          : <p className="mt-body-sm" style={{ color: 'var(--t-faint)' }}>Loading…</p>}
      </div>
    )
  }

  const dateLabel = new Date(`${data.date}T00:00:00`).toLocaleDateString('en-US', { weekday: 'long', month: 'short', day: 'numeric' })
  // Nothing to pick from AND nothing picked. Rather than the old "connect a tracker"
  // dead end, this is where someone with no tracker writes their first task — the
  // composer IS the empty state, and the tracker line is demoted to a footnote.
  const boardEmpty = (data.available?.length ?? 0) === 0 && today.length === 0

  return (
    <div className="h-full flex flex-col min-h-0">
    <DragDropContext onDragStart={() => { draggingRef.current = true; pausePlanRefresh(true) }} onDragEnd={onDragEnd}>
      <div className="flex-1 min-h-0 flex flex-col">
        <header className="shrink-0 flex items-center justify-between gap-4 px-6 pt-5 pb-4 border-b" style={{ borderColor: 'var(--t-hair)' }}>
          <p className="mt-label" style={{ color: 'var(--t-faint)' }}>{dateLabel}</p>
          <div className="text-right shrink-0">
            {confirmedMode
              ? (editing
                ? <p className="mt-body-sm" style={{ color: 'var(--color-state-proposal)' }}>Editing · unsaved changes</p>
                : <p className="mt-body-sm" style={{ color: 'var(--color-state-approved)' }}>Confirmed · {today.length} task{today.length === 1 ? '' : 's'}</p>)
              : skipped
                ? <p className="mt-body-sm" style={{ color: 'var(--t-faint)' }}>Skipped for today</p>
                : <p className="mt-body-sm" style={{ color: 'var(--t-faint)' }}>Pick your focus, then confirm</p>}
            {saveError && <p className="mt-body-sm mt-0.5" style={{ color: 'var(--color-state-pending)' }}>Couldn’t save — is the daemon running?</p>}
            {capHit && (
              <p className="mt-body-sm mt-0.5" style={{ color: 'var(--color-state-pending)' }}>
                {MAX_PLAN_TASKS} tasks is the limit for one day - remove one to add another.
              </p>
            )}
          </div>
        </header>

        {boardEmpty ? (
          // Top-anchored + scrollable, NOT vertically centered: centering a flex
          // item taller than the viewport clips it symmetrically top and bottom
          // (the classic unsafe-centering bug) — the footer "Add to today" button
          // was getting pushed past the scrollable edge and clipped by the modal.
          // Every other modal in this codebase (SettingsModal, ReportModal) is
          // top-anchored inside its scroll region; this follows the same convention.
          <div className="flex-1 min-h-0 overflow-y-auto nice-scroll p-6">
            <TaskComposer hero day={todayKey} trackers={trackers} onDone={onCreated} />
            {trackers.length === 0 && (
              <p className="mt-4 text-center mt-body-sm mx-auto" style={{ color: 'var(--t-faint-2)', maxWidth: 560 }}>
                Using Jira, Linear or GitHub? Connect it in Settings and your tickets show up here too.
              </p>
            )}
          </div>
        ) : (
          <div className="grid grid-cols-1 lg:grid-cols-2 gap-5 flex-1 min-h-0 p-6">
            <PlanTodayColumn
              today={today} editable={editable} proposed={proposed} editing={editing}
              confirmedMode={confirmedMode} skipped={skipped}
              onRemove={key => commit(today.filter(x => x.key !== key))}
              onOpen={setOpenTask}
              onConfirm={() => metaAction('confirm', today.map(t => t.key))}
              onSkip={() => metaAction('skip', [])}
              onReopen={() => metaAction('reopen', [])}
              onEdit={() => setEditing(true)}
              onSave={saveEdits}
              onCancel={cancelEdits}
            />
            <PlanBoardColumn
              day={todayKey} board={board} trackers={trackers} editable={editable}
              planFull={today.length >= MAX_PLAN_TASKS}
              search={search} sortMode={sortMode}
              onSearch={setSearch} onSort={setSortMode}
              onAdd={t => commit([...today, t])}
              onOpen={setOpenTask}
              onCreated={onCreated}
            />
          </div>
        )}
      </div>

      {openTask && (
        <TaskDetailDialog
          taskKey={openTask.key}
          fallbackTitle={openTask.title}
          inToday={today.some(t => t.key === openTask.key)}
          canEdit={editable}
          onClose={() => setOpenTask(null)}
          onAdd={() => commit([...today, openTask])}
          onRemove={() => commit(today.filter(t => t.key !== openTask.key))}
        />
      )}
    </DragDropContext>
    </div>
  )
}
