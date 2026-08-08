//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// The morning planner's shell. It owns four things and delegates everything else:
//   - the day key + the shared plan store subscription,
//   - the DnD context and `onDragEnd` (which moves cards BETWEEN the two columns, so
//     the `today` list can't live in either one of them),
//   - the skipped/active state (the old proposed/editing/confirmed algebra went
//     with the Confirm button - see `commit`),
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
import { syncTasks } from '@/lib/taskSync'
import { availableTrackerNames, connectedTrackers } from '@/lib/integrations'
import { usePlan, refreshPlan, planAction, pausePlanRefresh } from '@/components/plan/planStore'
import { isTutorialRunning } from '@/components/tutorial/engine'

export default function PlanView() {
  // The planner always edits TODAY (the timeline gates "Edit plan" to today), so
  // it keys the shared store by today's local day — the same key OverviewPanel
  // uses while viewing today, which is what makes a confirm/save here land in
  // its "Today's focus" instantly. Fixed at mount: the planner is a modal, so a
  // midnight rollover re-keys on the next open.
  const todayKey = useMemo(() => dayString(0), [])
  // Server truth lives in the shared store; `today` mirrors it and every change to
  // it writes straight back through `commit`.
  const { data } = usePlan(todayKey)
  const [loadFailed, setLoadFailed] = useState(false)
  const [today, setToday] = useState<CardTask[]>([])
  const [skipped, setSkipped] = useState(false)
  const [search, setSearch] = useState('')
  const [sortMode, setSortMode] = useState<'top' | 'due' | 'az'>('top')
  const [saveError, setSaveError] = useState(false)
  // An add was refused for hitting the plan cap. Cleared by the next successful
  // commit, so it never lingers past the thing it describes.
  const [capHit, setCapHit] = useState(false)
  const [openTask, setOpenTask] = useState<CardTask | null>(null)
  const [trackers, setTrackers] = useState<ReturnType<typeof connectedTrackers>>([])
  // Whether the integrations read has come back yet. Distinct from `trackers`
  // being empty, which is also what it looks like for the first few frames -
  // and the layout below turns on exactly that distinction.
  const [trackersLoaded, setTrackersLoaded] = useState(false)
  // Flipped once the first-sync backstop has run to completion (or decided not to
  // run). State, not the ref that guards it, because the layout reads it.
  const [firstSyncDone, setFirstSyncDone] = useState(false)
  const [syncing, setSyncing] = useState(false)
  const [syncError, setSyncError] = useState(false)
  const draggingRef = useRef(false)

  const derive = useCallback((d: PlanResponse) => {
    // `confirmed` is still read here to decide whether the server has a plan worth
    // restoring, but it is no longer a MODE - nothing renders differently for it,
    // because saving and confirming are now the same act. The state that mirrored
    // it went with the Confirm button.
    const isConfirmed = d.confirmed && !d.skipped
    setSkipped(d.skipped)
    const avail = new Map(d.available.map(a => [a.key, a]))
    if (isConfirmed || d.plan.length > 0) setToday(d.plan.map(p => fromPlan(p, avail)))
    // The suggestion pre-fill, SUPPRESSED while the walkthrough is driving.
    //
    // Seeding Today with what looks active is right on an ordinary morning - it
    // saves the user the first drag. It is wrong in a tour, because the very next
    // beat asks them to drag today's work across, and the app has already done it
    // for them: they are asked to perform a move they can watch has been made.
    // Worse, on a fresh install the suggestions come from a board that may hold
    // one stale ticket, so the demonstration opens with a task the user does not
    // recognise sitting in their plan.
    //
    // Only the SEEDING is skipped. A plan that actually exists (`d.plan.length`,
    // above) still loads - a replay on a real working day must show real state,
    // not an empty column.
    else if (!d.skipped) setToday(isTutorialRunning() ? [] : d.suggestions.map(fromAvailable))
    else setToday([])
  }, [])

  // `initial` (mount / error-rollback) re-seeds the Today list from the server.
  // A background poll passes initial=false: it refreshes the board (so a PM sync's
  // new/changed tickets appear) WITHOUT re-deriving Today, so it never clobbers the
  // user's in-progress edits. Skipped entirely during an active drag — the store
  // holds off background refreshes for the drag's duration (see pausePlanRefresh),
  // which also stops OTHER readers' polls from swapping the board mid-drag.
  //
  // RESOLVES THE OUTCOME (`true` = the board is current). `refreshPlan` swallows
  // its own failure and resolves `null` rather than throwing, so a caller that
  // needs to know cannot learn it from a rejection - it has to come back as a
  // value. The Refresh chip below depends on this.
  const load = useCallback((initial = false): Promise<boolean> => {
    // Nothing was attempted, so nothing failed - a paused background poll is not
    // an error and must not light up the chip.
    if (!initial && draggingRef.current) return Promise.resolve(true)
    // A failed read = real backend failure (not an empty day) → surface, don't
    // render empty. `refreshPlan` publishes to the store, so `data` arrives via
    // usePlan rather than local state.
    return refreshPlan(todayKey).then(d => {
      if (!initial) return d != null
      if (d) { setLoadFailed(false); derive(d); return true }
      setLoadFailed(true)
      return false
    })
  }, [derive, todayKey])

  useEffect(() => {
    load(true)
    const id = setInterval(() => load(false), 30_000)
    return () => clearInterval(id)
  }, [load])

  // Pull the latest tickets from every connected tracker before re-deriving the
  // board — same `sync_tasks` command and refresh-after pattern as the Tasks
  // page's own Refresh button (TasksPanel.tsx's handleSync). Only meaningful
  // with a tracker connected: personal-only plans have nothing to sync.
  // Returns the in-flight promise so the first-sync backstop below can tell when
  // the wait is genuinely over rather than guessing at a delay.
  //
  // Goes through `syncTasks`, which JOINS a sync already started by the connect flow
  // rather than firing a second one. That matters on the path this exists for: the
  // connect page now warm-starts the first sync, so by the time the planner mounts
  // the call is usually already in flight and half done. Firing our own would spend a
  // second outward request against the user's rate limit and then wait out the slower
  // of the two.
  //
  // THE FAILURE SIGNAL IS THE RESULT, NOT A REJECTION. Neither `syncTasks` nor
  // `load` ever rejects - both swallow and report - so the `.catch` that used to
  // sit here was unreachable and the chip could never show "Sync failed" again.
  // Both outcomes are checked: the sync can fail while the read succeeds (the
  // board simply stays as it was), which is precisely the case worth telling the
  // user about, since nothing else on screen would change.
  const handleSync = useCallback(() => {
    if (syncing) return Promise.resolve()
    setSyncing(true); setSyncError(false)
    return syncTasks()
      .then(synced => load(true).then(loaded => { if (!synced || !loaded) setSyncError(true) }))
      .finally(() => setSyncing(false))
  }, [syncing, load])

  // Which trackers are connected decides whether the composer offers a "file it on
  // my board" choice at all. A failure here is not fatal: [] means personal-only,
  // which is the safe default (it never files anything outward).
  useEffect(() => {
    bridgeLoad<IntegrationsResponse>('/api/integrations', 'get_integrations')
      .then(r => setTrackers(connectedTrackers(r)))
      .catch(() => setTrackers([]))
      .finally(() => setTrackersLoaded(true))
  }, [])

  // FIRST-SYNC BACKSTOP: a connected tracker with an empty board pulls, once.
  //
  // `available` is read from the local DB, and a tracker connected sixty seconds
  // ago has not populated it yet - the daemon's own sync is on its own schedule.
  // So the planner opened straight off a fresh Jira connect saw zero tickets, took
  // the `boardEmpty` branch, and showed "write your first task by hand" to someone
  // who had just spent a minute importing a board. The tickets were real and
  // arriving; the one screen that exists to show them said there were none, and the
  // only way out was noticing the Refresh chip in the header.
  //
  // ONCE per mount, ref-guarded, and only when there is genuinely nothing to show -
  // this fires an outward API call per tracker, so it must never become a poll. A
  // board that is legitimately empty pays one request per planner open, which is
  // the right trade for never showing an imported board as empty.
  const autoSyncedRef = useRef(false)
  useEffect(() => {
    if (autoSyncedRef.current || !trackersLoaded || !data) return
    autoSyncedRef.current = true
    // Nothing to pull, or something already there to show: the wait is over
    // before it started.
    if (trackers.length === 0 || (data.available?.length ?? 0) > 0) { setFirstSyncDone(true); return }
    // Usually this JOINS the sync the connect flow already started - see `handleSync`.
    // It stays as a backstop because plenty of empty-board arrivals never went near a
    // connect: a reopened app, a board that emptied, a sync that failed an hour ago.
    handleSync().finally(() => setFirstSyncDone(true))
  }, [data, trackers, trackersLoaded, handleSync])

  // The drag hold-off is module-global, so a planner closed mid-drag (onDragEnd
  // never fires) would strand every reader's refresh paused for the rest of the
  // session. Always release it on unmount.
  useEffect(() => () => pausePlanRefresh(false), [])

  // `persist` (a bare 'set' write) lived here for the Save-changes button. With
  // every change writing through `commit` there is one write path, and it is the
  // one that also marks the plan confirmed - a 'set' that left `confirmed` alone
  // would quietly produce a saved-but-unconfirmed plan that nothing surfaces.
  const metaAction = useCallback((action: string, keys: string[]) => {
    planAction(todayKey, action, keys)
      .then(d => { setSaveError(false); derive(d) })
      .catch(() => setSaveError(true))
  }, [derive, todayKey])

  // EVERY CHANGE SAVES. Dropping a task into Today, removing one, reordering, or
  // adding from the composer writes the plan and marks it confirmed, immediately.
  //
  // This used to be local-only, with the write gated behind a Confirm button (and,
  // once confirmed, behind Edit plan → Save changes). That asked the user to state
  // an intention they had already acted out: dragging a ticket into a column
  // labelled "Today" IS saying it is today's work, and a second button to agree
  // with themselves is a step whose only real function was to let people lose
  // their plan by closing the modal before pressing it.
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
    // Optimistic: `today` renders from local state and the server re-derive that
    // follows only corrects it. A failed write raises the existing save banner
    // rather than yanking the card back out from under the cursor.
    metaAction('confirm', next.map(t => t.key))
  }, [today.length, metaAction])

  // Editable whenever the day is not skipped. There is no locked state left to be
  // in: a confirmed plan is just a saved one, and it stays as editable as it was a
  // second before it was saved. Keeping the old lock alongside auto-save would be
  // the worst of both - the first drag silently freezes the column, and the user
  // has to find "Edit plan" to make a second one.
  const editable = !skipped

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
  //
  // …but ONLY once we know there is nothing coming. `available` reads the local
  // DB, so for the first frames after a connect it is empty because the sync has
  // not run yet, not because the board is. Committing to this layout then flashed
  // the hero composer - "write your first task by hand" - at someone who had just
  // finished importing a Jira project, for the second or two before their tickets
  // landed and it swapped to the two columns underneath. A layout that changes its
  // mind about what the user's setup IS reads as the connect having half-failed.
  //
  // While that is unknown the two-column layout renders instead, with the board
  // column saying it is pulling: that is where they are going to end up, so it is
  // the honest thing to show, and nothing has to move when the tickets arrive.
  const boardEmpty = (data.available?.length ?? 0) === 0 && today.length === 0
    && trackersLoaded && firstSyncDone

  return (
    <div className="h-full flex flex-col min-h-0">
    <DragDropContext onDragStart={() => { draggingRef.current = true; pausePlanRefresh(true) }} onDragEnd={onDragEnd}>
      <div className="flex-1 min-h-0 flex flex-col">
        <header className="shrink-0 flex items-center justify-between gap-4 px-6 pt-5 pb-4 border-b" style={{ borderColor: 'var(--t-hair)' }}>
          <div className="flex items-center gap-3">
            <p className="mt-label" style={{ color: 'var(--t-faint)' }}>{dateLabel}</p>
            {/* Only meaningful with a tracker connected — a personal-only plan
                has nothing on a remote board to pull. Same sync + button
                treatment as the Tasks page's Refresh (TasksPanel.tsx). */}
            {trackers.length > 0 && (
              <button onClick={handleSync} disabled={syncing}
                className="mt-body-sm px-2.5 py-1 rounded-md bg-ctrl inline-flex items-center gap-1.5"
                style={{ border: `1px solid ${syncError ? 'var(--color-state-pending)' : 'var(--t-ctrl-border)'}`, color: syncError ? 'var(--color-state-pending)' : 'var(--t-muted)', opacity: syncing ? 0.6 : 1 }}
                title="Pull latest tickets from your trackers">
                <span style={{ display: 'inline-block', animation: syncing ? 'spin 1s linear infinite' : 'none' }}>{syncError ? '⚠' : '↻'}</span>
                {syncing ? 'Syncing…' : syncError ? 'Sync failed' : 'Refresh'}
              </button>
            )}
          </div>
          <div className="text-right shrink-0">
            {/* No "unsaved changes" state left to report, and no "then confirm"
                instruction: there is nothing to confirm. The line now says what is
                true - how many tasks are saved, or that the day was skipped. */}
            {skipped
              ? <p className="mt-body-sm" style={{ color: 'var(--t-faint)' }}>Skipped for today</p>
              : today.length > 0
                ? <p className="mt-body-sm" style={{ color: 'var(--color-state-approved)' }}>Saved · {today.length} task{today.length === 1 ? '' : 's'}</p>
                : <p className="mt-body-sm" style={{ color: 'var(--t-faint)' }}>Pick your focus for today</p>}
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
                Using {availableTrackerNames()}? Connect it in Settings and your tickets show up here too.
              </p>
            )}
          </div>
        ) : (
          <div className="grid grid-cols-1 lg:grid-cols-2 gap-5 flex-1 min-h-0 p-6">
            <PlanTodayColumn
              today={today} editable={editable} skipped={skipped}
              onRemove={key => commit(today.filter(x => x.key !== key))}
              onOpen={setOpenTask}
              onSkip={() => metaAction('skip', [])}
              onReopen={() => metaAction('reopen', [])}
            />
            <PlanBoardColumn
              day={todayKey} board={board} trackers={trackers} editable={editable}
              planFull={today.length >= MAX_PLAN_TASKS}
              pulling={!firstSyncDone || syncing}
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
          day={todayKey}
          inToday={today.some(t => t.key === openTask.key)}
          canEdit={editable}
          onClose={() => setOpenTask(null)}
          onAdd={() => commit([...today, openTask])}
          onRemove={() => commit(today.filter(t => t.key !== openTask.key))}
          onDeleted={() => commit(today.filter(t => t.key !== openTask.key))}
        />
      )}
    </DragDropContext>
    </div>
  )
}
