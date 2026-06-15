//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  DragDropContext, Droppable, Draggable,
  type DropResult, type DraggableProvided, type DraggableStateSnapshot,
} from '@hello-pangea/dnd'
import { Card } from '@/components/atoms'
import { GripHandle } from '@/components/plan/parts'
import { TaskCardBody, type CardTask } from '@/components/plan/TaskCard'
import TaskDialog from '@/components/plan/TaskDialog'
import type { PlanResponse, AvailableTask, PlanItem } from '@/lib/daily-plan'

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

// White & blue theme, scoped to the plan page — overrides the app's warm accent
// for all `var(--accent)` / `var(--accent-soft)` / `var(--tint)` usages below
// (and the dialog, which is a descendant). `--warn` → red so urgency isn't orange.
const BLUE_THEME = {
  '--accent': '#2563EB',
  '--accent-soft': '#E8F0FE',
  '--tint': 'rgba(37,99,235,0.06)',
  '--warn': '#DC2626',
} as React.CSSProperties

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
            borderColor: 'var(--rule)',
            background: 'var(--surface)',
            boxShadow: snapshot.isDragging ? '0 10px 30px rgba(0,0,0,0.18)' : 'none',
          }}
          className="rounded-lg border">
          <TaskCardBody task={task} detail={detail} onOpen={onOpen}
            lead={draggable
              ? (
                <span {...provided.dragHandleProps} aria-label={`Drag ${task.key}`}
                  className={`shrink-0 cursor-grab active:cursor-grabbing -ml-1 px-0.5 rounded inline-flex items-center ${FOCUS}`}
                  style={{ color: 'var(--ink-4)' }}>
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
    fetch('/api/plan').then(r => {
      if (!r.ok) throw new Error(`plan load failed: ${r.status}`)  // 500 = real backend error, not an empty day
      return r.json()
    }).then((d: PlanResponse) => {
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
    fetch('/api/plan', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ action: 'set', task_keys: keys }) })
      .then(r => { if (!r.ok) { setSaveError(true); load(true) } else setSaveError(false) })   // rollback to server truth
      .catch(() => { setSaveError(true); load(true) })
  }, [load])

  const metaAction = useCallback((action: string, keys: string[]) => {
    fetch('/api/plan', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ action, task_keys: keys }) })
      .then(async r => {
        if (!r.ok) { setSaveError(true); return }
        setSaveError(false)
        const d: PlanResponse = await r.json(); setData(d); derive(d)
      })
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
      <div className="space-y-8">
        <header className="rise"><h1 className="type-title" style={{ color: 'var(--ink)' }}>Today&apos;s plan</h1></header>
        {loadFailed
          ? <button onClick={() => load(true)} className="text-[13px] px-3 py-2 rounded-md" style={{ background: 'var(--accent)', color: '#fff' }}>Couldn’t load — retry</button>
          : <p className="text-[13px]" style={{ color: 'var(--ink-3)' }}>Loading…</p>}
      </div>
    )
  }

  const dateLabel = new Date(`${data.date}T00:00:00`).toLocaleDateString('en-US', { weekday: 'long', month: 'long', day: 'numeric' })
  const boardEmpty = (data.available?.length ?? 0) === 0 && today.length === 0

  return (
    <div style={BLUE_THEME} className="h-full flex flex-col min-h-0">
    <DragDropContext onDragStart={() => { draggingRef.current = true }} onDragEnd={onDragEnd}>
      <div className="flex-1 min-h-0 flex flex-col gap-8">
        <header className="rise shrink-0 flex items-end justify-between gap-4">
          <div>
            <p className="text-[11px] uppercase tracking-[0.2em]" style={{ color: 'var(--ink-3)' }}>{dateLabel}</p>
            <h1 className="type-title mt-1" style={{ color: 'var(--ink)' }}>What are you working on today?</h1>
          </div>
          <div className="text-right shrink-0">
            {confirmedMode
              ? (editing
                ? <p className="text-[12px]" style={{ color: 'var(--accent)' }}>Editing · unsaved changes</p>
                : <p className="text-[12px]" style={{ color: 'var(--success)' }}>Confirmed · {today.length} task{today.length === 1 ? '' : 's'}</p>)
              : skipped
                ? <p className="text-[12px]" style={{ color: 'var(--ink-3)' }}>Skipped for today</p>
                : <p className="text-[12px]" style={{ color: 'var(--ink-3)' }}>Pick your focus, then confirm</p>}
            {saveError && <p className="text-[11px] mt-0.5" style={{ color: 'var(--warn)' }}>Couldn’t save — is the daemon running?</p>}
          </div>
        </header>

        {boardEmpty ? (
          <div className="py-16 text-center rounded-xl border" style={{ borderColor: 'var(--rule)', background: 'var(--surface)' }}>
            <p className="type-empty" style={{ color: 'var(--ink-2)' }}>No tasks on your board yet.</p>
            <p className="text-[12px] mt-2" style={{ color: 'var(--ink-3)' }}>Connect a tracker in Settings and your tickets will appear here.</p>
          </div>
        ) : (
          <div className="grid grid-cols-1 lg:grid-cols-2 gap-6 items-stretch flex-1 min-h-0">
            {/* ── LEFT: Today ──────────────────────────────────────────────── */}
            <Card className="p-5 flex flex-col min-h-0">
              <div className="flex items-center justify-between mb-1">
                <p className="text-[10px] uppercase tracking-[0.18em]" style={{ color: 'var(--ink-3)' }}>Today · {today.length}</p>
                {proposed && today.length > 0 && (
                  <span className="text-[10px] px-1.5 py-0.5 rounded-md" style={{ color: 'var(--accent)', background: 'var(--accent-soft)' }}>Suggested</span>
                )}
              </div>
              <p className="text-[12px] mb-2" style={{ color: 'var(--ink-3)' }}>
                {proposed
                  ? 'We pre-filled what looks active. Drag to reorder, drag a card out to remove, or add from your board — then confirm.'
                  : editing
                    ? 'Reorder, add, or remove — then Save to update today’s plan, or Cancel to discard.'
                    : confirmedMode
                      ? 'Your plan is locked in. Hit Edit plan to make changes.'
                      : 'Plan your day below.'}
              </p>
              {editable && today.length > 5 && (
                <p className="text-[11px] mb-3 flex items-center gap-1.5" style={{ color: 'var(--warn)' }}>
                  <span>⚠</span> That&apos;s a full plate — most focused days land on 1–3 tasks.
                </p>
              )}

              <Droppable droppableId="today">
                {(provided, snapshot) => (
                  <div ref={provided.innerRef} {...provided.droppableProps}
                    className="rounded-xl transition-colors flex-1 min-h-0 overflow-y-auto p-2 space-y-2"
                    style={{ background: snapshot.isDraggingOver ? 'var(--accent-soft)' : 'var(--surface-2)', outline: snapshot.isDraggingOver ? '1.5px dashed var(--accent)' : '1.5px dashed transparent', outlineOffset: 2 }}>
                    {today.map((t, i) => (
                      <DraggableCard key={t.key} task={t} index={i} draggable={editable} onOpen={() => setOpenTask(t)}
                        trail={
                          <div className="flex items-center gap-1 shrink-0">
                            <span aria-hidden className="font-mono tnum text-[11px] mr-1" style={{ color: 'var(--ink-4)' }}>{i + 1}</span>
                            {editable && (
                              <button onClick={() => commit(today.filter(x => x.key !== t.key))} aria-label={`Remove ${t.key} from today`}
                                className={`w-6 h-6 rounded-md flex items-center justify-center transition-colors hover:bg-[var(--rule)] ${FOCUS}`}
                                style={{ color: 'var(--ink-4)' }}>
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
                        <p className="text-[12px]" style={{ color: 'var(--ink-4)' }}>
                          {editable
                            ? <>Drag tasks here, or tap <span style={{ color: 'var(--accent)' }}>+ Add</span> from your board.</>
                            : 'No tasks in today’s plan.'}
                        </p>
                      </div>
                    )}
                  </div>
                )}
              </Droppable>

              <div className="mt-5 pt-4 rule-t shrink-0 flex items-center gap-3" style={{ borderTopColor: 'var(--rule)' }}>
                {proposed ? (
                  <>
                    <button onClick={() => metaAction('confirm', today.map(t => t.key))}
                      className={`px-4 py-2 rounded-md text-[13px] font-medium transition-colors ${FOCUS}`}
                      style={{ background: 'var(--accent)', color: '#fff' }}>
                      Confirm {today.length > 0 ? `${today.length} task${today.length === 1 ? '' : 's'}` : 'plan'} →
                    </button>
                    <button onClick={() => metaAction('skip', [])} className={`text-[12px] px-2 py-1.5 rounded-md ml-auto ${FOCUS}`} style={{ color: 'var(--ink-4)' }}>
                      Skip today
                    </button>
                  </>
                ) : skipped ? (
                  <button onClick={() => metaAction('reopen', [])}
                    className={`px-4 py-2 rounded-md text-[13px] font-medium transition-colors ${FOCUS}`}
                    style={{ background: 'var(--accent)', color: '#fff' }}>
                    Plan today →
                  </button>
                ) : editing ? (
                  <>
                    <button onClick={saveEdits}
                      className={`px-4 py-2 rounded-md text-[13px] font-medium transition-colors ${FOCUS}`}
                      style={{ background: 'var(--accent)', color: '#fff' }}>
                      Save changes
                    </button>
                    <button onClick={cancelEdits} className={`text-[12px] px-2 py-1.5 rounded-md ${FOCUS}`} style={{ color: 'var(--ink-4)' }}>
                      Cancel
                    </button>
                  </>
                ) : (
                  <>
                    <button onClick={() => setEditing(true)}
                      className={`px-4 py-2 rounded-md text-[13px] font-medium border transition-colors ${FOCUS}`}
                      style={{ borderColor: 'var(--rule)', color: 'var(--ink-2)', background: 'var(--paper)' }}>
                      Edit plan
                    </button>
                    <span className="text-[12px] ml-auto" style={{ color: 'var(--ink-4)' }}>These lead today’s task matching.</span>
                  </>
                )}
              </div>
            </Card>

            {/* ── RIGHT: Your tasks (board) ─────────────────────────────────── */}
            <Card className="p-5 flex flex-col min-h-0">
              <p className="text-[10px] uppercase tracking-[0.18em] mb-3 shrink-0" style={{ color: 'var(--ink-3)' }}>
                Your tasks{search ? ` · ${board.length} match${board.length === 1 ? '' : 'es'}` : ` · ${board.length}`}
              </p>
              <div className="flex items-center gap-2 mb-4 shrink-0">
                <input
                  value={search} onChange={e => setSearch(e.target.value)} aria-label="Search your tasks"
                  placeholder="Search tasks…"
                  className={`flex-1 text-[13px] px-3 py-2 rounded-md ${FOCUS}`}
                  style={{ background: 'var(--surface-2)', color: 'var(--ink)', border: '1px solid var(--rule)' }}
                />
                <div role="radiogroup" aria-label="Sort tasks" className="flex rounded-md overflow-hidden" style={{ border: '1px solid var(--rule)' }}>
                  {(['top', 'due', 'az'] as const).map(m => (
                    <button key={m} role="radio" aria-checked={sortMode === m} onClick={() => setSortMode(m)}
                      className={`text-[11px] px-2.5 py-2 transition-colors ${FOCUS}`}
                      style={{ background: sortMode === m ? 'var(--surface-2)' : 'var(--surface)', color: sortMode === m ? 'var(--ink)' : 'var(--ink-3)' }}>
                      {m === 'top' ? 'Top' : m === 'due' ? 'Due' : 'A–Z'}
                    </button>
                  ))}
                </div>
              </div>

              <Droppable droppableId="board">
                {(provided, snapshot) => (
                  <div ref={provided.innerRef} {...provided.droppableProps}
                    className="space-y-2 flex-1 min-h-0 overflow-y-auto rounded-xl transition-colors p-1 pr-1.5"
                    style={{ outline: snapshot.isDraggingOver ? '1.5px dashed var(--rule-2)' : '1.5px dashed transparent', outlineOffset: 2 }}>
                    {board.map((t, i) => (
                      <DraggableCard key={t.key} task={t} index={i} detail draggable={editable} onOpen={() => setOpenTask(t)}
                        trail={editable
                          ? (
                            <button onClick={() => commit([...today, t])} aria-label={`Add ${t.key} to today`}
                              className={`shrink-0 px-3 py-1.5 rounded-md text-[12px] font-medium transition-colors hover:opacity-80 ${FOCUS}`}
                              style={{ background: 'var(--accent-soft)', color: 'var(--accent)' }}>
                              + Add
                            </button>
                          )
                          : undefined}
                      />
                    ))}
                    {provided.placeholder}
                    {board.length === 0 && (
                      <p className="py-8 text-center text-[12px]" style={{ color: 'var(--ink-4)' }}>
                        {search ? 'No tasks match your search.' : 'Everything is in today’s plan. Drag a card here to remove it.'}
                      </p>
                    )}
                  </div>
                )}
              </Droppable>
            </Card>
          </div>
        )}
      </div>

      {openTask && (
        <TaskDialog
          task={openTask}
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
