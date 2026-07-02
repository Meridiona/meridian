//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  DragDropContext, Droppable, Draggable,
  type DropResult, type DraggableProvided, type DraggableStateSnapshot,
} from '@hello-pangea/dnd'
import { GripHandle } from '@/components/plan/parts'
import { TaskCardBody, type CardTask } from '@/components/plan/TaskCard'
import { TaskDetailDialog } from '@/components/timeline/TaskDetailDialog'
import type { PlanResponse, AvailableTask, PlanItem } from '@/lib/api-types'
import { load as loadBridge, mutate } from '@/lib/bridge'

// ── normalisation ────────────────────────────────────────────────────────────
function fromAvailable(a: AvailableTask): CardTask {
  return {
    key: a.key, title: a.title, provider: a.provider, url: a.url, due_days: a.due_days,
    reason: a.reason, origin: a.origin, is_terminal: a.is_terminal,
    description: a.description, epic: a.epic, status: a.status, priority: a.priority,
    issue_type: a.issue_type, story_points: a.story_points,
  }
}
const REASON: Record<string, string> = {
  carryover: 'Carried over', in_progress: 'In progress', due_soon: 'Due soon', recent: 'Worked recently', manual: 'Added',
}
function fromPlan(p: PlanItem, avail: Map<string, AvailableTask>): CardTask {
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

const FOCUS = 'outline-none focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2'
const PRIMARY_BTN = 'mt-body-sm px-4 py-2 rounded-lg font-semibold transition-opacity hover:opacity-90'
const GHOST_BTN = 'mt-body-sm px-4 py-2 rounded-lg bg-ctrl transition-opacity hover:opacity-80'

// A card body that's draggable when `draggable` is set (a grip handle appears and
// the row can be dragged) + caller-supplied trailing controls. When locked
// (`draggable=false`) the handle is hidden and the row can't be dragged. Shared by
// both columns.
function DraggableCard({
  task, index, trail, detail = false, onOpen, draggable = true,
}: { task: CardTask; index: number; trail?: React.ReactNode; detail?: boolean; onOpen?: () => void; draggable?: boolean }) {
  return (
    <Draggable draggableId={task.key} index={index} isDragDisabled={!draggable}>
      {(provided: DraggableProvided, snapshot: DraggableStateSnapshot) => (
        <div ref={provided.innerRef} {...provided.draggableProps}
          style={{
            ...provided.draggableProps.style,
            borderColor: 'var(--t-card-border)',
            background: 'var(--t-card)',
            boxShadow: snapshot.isDragging ? '0 16px 32px -12px rgba(40,30,90,0.32)' : 'none',
          }}
          className="rounded-lg border">
          <TaskCardBody task={task} detail={detail} onOpen={onOpen}
            lead={draggable
              ? (
                <span {...provided.dragHandleProps} aria-label={`Drag ${task.key}`}
                  className={`shrink-0 cursor-grab active:cursor-grabbing -ml-1 px-0.5 rounded inline-flex items-center ${FOCUS}`}
                  style={{ color: 'var(--t-faint-2)' }}>
                  <GripHandle />
                </span>
              )
              : undefined}
            trail={trail}
          />
        </div>
      )}
    </Draggable>
  )
}

// ── main view ────────────────────────────────────────────────────────────────
export default function PlanView() {
  const [data, setData] = useState<PlanResponse | null>(null)
  const [loadFailed, setLoadFailed] = useState(false)
  const [today, setToday] = useState<CardTask[]>([])
  const [confirmedMode, setConfirmedMode] = useState(false)
  const [editing, setEditing] = useState(false)
  const [skipped, setSkipped] = useState(false)
  const [search, setSearch] = useState('')
  const [sortMode, setSortMode] = useState<'top' | 'due' | 'az'>('top')
  const [saveError, setSaveError] = useState(false)
  const [openTask, setOpenTask] = useState<CardTask | null>(null)
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
  // user's in-progress edits. Skipped entirely during an active drag.
  const load = useCallback((initial = false) => {
    if (!initial && draggingRef.current) return
    // Dual-path: get_plan (Rust) in the app, /api/plan in a browser. A thrown
    // error = real backend failure (not an empty day) → surface, don't render empty.
    loadBridge<PlanResponse>('/api/plan', 'get_plan').then((d: PlanResponse) => {
      setData(d); setLoadFailed(false)
      if (initial) derive(d)
    }).catch(() => { if (initial) setLoadFailed(true) })
  }, [derive])

  useEffect(() => {
    load(true)
    const id = setInterval(() => load(false), 30_000)
    return () => clearInterval(id)
  }, [load])

  // Persist a Today ordering (live mode only); roll back to server truth on error.
  const persist = useCallback((keys: string[]) => {
    mutate('/api/plan', 'plan_action', { action: 'set', task_keys: keys })
      .then(() => setSaveError(false))
      .catch(() => { setSaveError(true); load(true) })   // rollback to server truth
  }, [load])

  const metaAction = useCallback((action: string, keys: string[]) => {
    mutate<PlanResponse>('/api/plan', 'plan_action', { action, task_keys: keys })
      .then(d => { setSaveError(false); setData(d); derive(d) })
      .catch(() => setSaveError(true))
  }, [derive])

  // Local edit only — both the proposed draft and the "Edit plan" session keep
  // changes in `today` until the dev explicitly Confirms / Saves. No silent writes.
  const commit = useCallback((next: CardTask[]) => {
    setToday(next)
  }, [])

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
  const boardEmpty = (data.available?.length ?? 0) === 0 && today.length === 0

  return (
    <div className="h-full flex flex-col min-h-0">
    <DragDropContext onDragStart={() => { draggingRef.current = true }} onDragEnd={onDragEnd}>
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
          </div>
        </header>

        {boardEmpty ? (
          <div className="flex-1 flex items-center justify-center p-6">
            <div className="py-16 px-8 text-center rounded-xl w-full" style={{ border: '1px dashed var(--t-hair)' }}>
              <p className="mt-title" style={{ color: 'var(--t-title)' }}>No tasks on your board yet.</p>
              <p className="mt-body-sm mt-2" style={{ color: 'var(--t-faint)' }}>Connect a tracker in Settings and your tickets will appear here.</p>
            </div>
          </div>
        ) : (
          <div className="grid grid-cols-1 lg:grid-cols-2 gap-5 flex-1 min-h-0 p-6">
            {/* ── LEFT: Today ──────────────────────────────────────────────── */}
            <div className="rounded-xl flex flex-col min-h-0 bg-card" style={{ border: '1px solid var(--t-card-border)' }}>
              <div className="shrink-0 px-4 pt-4">
                <div className="flex items-center justify-between mb-1">
                  <p className="mt-label" style={{ color: 'var(--t-faint)' }}>Today · {today.length}</p>
                  {proposed && today.length > 0 && (
                    <span className="mt-chip px-1.5 py-0.5 rounded"
                      style={{ color: 'var(--color-state-proposal)', background: 'color-mix(in srgb, var(--color-state-proposal) 12%, transparent)' }}>
                      Suggested
                    </span>
                  )}
                </div>
                <p className="mt-body-sm mb-2" style={{ color: 'var(--t-faint)' }}>
                  {proposed
                    ? 'We pre-filled what looks active. Drag to reorder, drag a card out to remove, or add from your board — then confirm.'
                    : editing
                      ? 'Reorder, add, or remove — then Save to update today’s plan, or Cancel to discard.'
                      : confirmedMode
                        ? 'Your plan is locked in. Hit Edit plan to make changes.'
                        : 'Plan your day below.'}
                </p>
                {editable && today.length > 5 && (
                  <p className="mt-body-sm mb-2 flex items-center gap-1.5" style={{ color: 'var(--color-state-pending)' }}>
                    <span>⚠</span> That&apos;s a full plate — most focused days land on 1–3 tasks.
                  </p>
                )}
              </div>

              <Droppable droppableId="today">
                {(provided, snapshot) => (
                  <div ref={provided.innerRef} {...provided.droppableProps}
                    className="rounded-xl transition-colors flex-1 min-h-0 overflow-y-auto nice-scroll mx-4 mb-2 p-2 space-y-2"
                    style={{
                      background: snapshot.isDraggingOver ? 'color-mix(in srgb, var(--color-state-proposal) 10%, transparent)' : 'var(--t-box)',
                      outline: snapshot.isDraggingOver ? '1.5px dashed var(--color-state-proposal)' : '1.5px dashed transparent', outlineOffset: 2,
                    }}>
                    {today.map((t, i) => (
                      <DraggableCard key={t.key} task={t} index={i} draggable={editable} onOpen={() => setOpenTask(t)}
                        trail={
                          <div className="flex items-center gap-1 shrink-0">
                            <span aria-hidden className="mt-mono-sm text-[11px] mr-1" style={{ color: 'var(--t-faint-2)' }}>{i + 1}</span>
                            {editable && (
                              <button onClick={() => commit(today.filter(x => x.key !== t.key))} aria-label={`Remove ${t.key} from today`}
                                className={`w-6 h-6 rounded-md flex items-center justify-center transition-colors hover:bg-wrap ${FOCUS}`}
                                style={{ color: 'var(--t-faint-2)' }}>
                                <svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6"><path d="M4 4l8 8M12 4l-8 8" /></svg>
                              </button>
                            )}
                          </div>
                        }
                      />
                    ))}
                    {provided.placeholder}
                    {today.length === 0 && !snapshot.isDraggingOver && (
                      <div className="py-9 text-center">
                        <p className="mt-body-sm" style={{ color: 'var(--t-faint-2)' }}>
                          {editable
                            ? <>Drag tasks here, or tap <span style={{ color: 'var(--color-state-proposal)' }}>+ Add</span> from your board.</>
                            : 'No tasks in today’s plan.'}
                        </p>
                      </div>
                    )}
                  </div>
                )}
              </Droppable>

              <div className="shrink-0 px-4 py-3.5 border-t flex items-center gap-3" style={{ borderColor: 'var(--t-hair)' }}>
                {proposed ? (
                  <>
                    <button onClick={() => metaAction('confirm', today.map(t => t.key))}
                      className={`${PRIMARY_BTN} ${FOCUS}`} style={{ background: 'var(--color-state-approved)', color: '#fff' }}>
                      Confirm {today.length > 0 ? `${today.length} task${today.length === 1 ? '' : 's'}` : 'plan'} →
                    </button>
                    <button onClick={() => metaAction('skip', [])} className={`mt-body-sm px-2 py-1.5 rounded-md ml-auto ${FOCUS}`} style={{ color: 'var(--t-faint-2)' }}>
                      Skip today
                    </button>
                  </>
                ) : skipped ? (
                  <button onClick={() => metaAction('reopen', [])}
                    className={`${PRIMARY_BTN} ${FOCUS}`} style={{ background: 'var(--color-state-approved)', color: '#fff' }}>
                    Plan today →
                  </button>
                ) : editing ? (
                  <>
                    <button onClick={saveEdits} className={`${PRIMARY_BTN} ${FOCUS}`} style={{ background: 'var(--color-state-approved)', color: '#fff' }}>
                      Save changes
                    </button>
                    <button onClick={cancelEdits} className={`mt-body-sm px-2 py-1.5 rounded-md ${FOCUS}`} style={{ color: 'var(--t-faint-2)' }}>
                      Cancel
                    </button>
                  </>
                ) : (
                  <>
                    <button onClick={() => setEditing(true)} className={`${GHOST_BTN} ${FOCUS}`}
                      style={{ border: '1px solid var(--t-ctrl-border)', color: 'var(--t-muted)' }}>
                      Edit plan
                    </button>
                    <span className="mt-body-sm ml-auto" style={{ color: 'var(--t-faint-2)' }}>These lead today’s task matching.</span>
                  </>
                )}
              </div>
            </div>

            {/* ── RIGHT: Your tasks (board) ─────────────────────────────────── */}
            <div className="rounded-xl flex flex-col min-h-0 bg-card" style={{ border: '1px solid var(--t-card-border)' }}>
              <div className="shrink-0 px-4 pt-4">
                <p className="mt-label mb-3" style={{ color: 'var(--t-faint)' }}>
                  Your tasks{search ? ` · ${board.length} match${board.length === 1 ? '' : 'es'}` : ` · ${board.length}`}
                </p>
                <div className="flex items-center gap-2 mb-3">
                  <input
                    value={search} onChange={e => setSearch(e.target.value)} aria-label="Search your tasks"
                    placeholder="Search tasks…"
                    className={`flex-1 mt-body-sm px-3 py-2 rounded-md ${FOCUS}`}
                    style={{ background: 'var(--t-input)', color: 'var(--t-title)', border: '1px solid var(--t-input-border)' }}
                  />
                  <div role="radiogroup" aria-label="Sort tasks" className="flex rounded-md overflow-hidden" style={{ border: '1px solid var(--t-ctrl-border)' }}>
                    {(['top', 'due', 'az'] as const).map(m => (
                      <button key={m} role="radio" aria-checked={sortMode === m} onClick={() => setSortMode(m)}
                        className={`mt-chip px-2.5 py-2 transition-colors ${FOCUS}`}
                        style={{ background: sortMode === m ? 'var(--t-wrap)' : 'var(--t-ctrl)', color: sortMode === m ? 'var(--t-title)' : 'var(--t-faint)' }}>
                        {m === 'top' ? 'Top' : m === 'due' ? 'Due' : 'A–Z'}
                      </button>
                    ))}
                  </div>
                </div>
              </div>

              <Droppable droppableId="board">
                {(provided, snapshot) => (
                  <div ref={provided.innerRef} {...provided.droppableProps}
                    className="space-y-2 flex-1 min-h-0 overflow-y-auto nice-scroll rounded-xl transition-colors mx-4 mb-4 p-1 pr-1.5"
                    style={{ outline: snapshot.isDraggingOver ? '1.5px dashed var(--t-hair)' : '1.5px dashed transparent', outlineOffset: 2 }}>
                    {board.map((t, i) => (
                      <DraggableCard key={t.key} task={t} index={i} detail draggable={editable} onOpen={() => setOpenTask(t)}
                        trail={editable
                          ? (
                            <button onClick={() => commit([...today, t])} aria-label={`Add ${t.key} to today`}
                              className={`shrink-0 mt-chip px-3 py-1.5 rounded-md transition-colors hover:opacity-80 ${FOCUS}`}
                              style={{ background: 'color-mix(in srgb, var(--color-state-approved) 14%, transparent)', color: 'var(--color-state-approved)' }}>
                              + Add
                            </button>
                          )
                          : undefined}
                      />
                    ))}
                    {provided.placeholder}
                    {board.length === 0 && (
                      <p className="py-8 text-center mt-body-sm" style={{ color: 'var(--t-faint-2)' }}>
                        {search ? 'No tasks match your search.' : 'Everything is in today’s plan. Drag a card here to remove it.'}
                      </p>
                    )}
                  </div>
                )}
              </Droppable>
            </div>
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
