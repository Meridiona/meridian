//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// The right "Your tasks" column: search + sort + the board list, and the ＋ New task
// composer that replaces all of it while you're writing one. Extracted from PlanView.
//
// The composer REPLACES the list rather than sitting above it: writing a task is a
// focused act, and a half-typed note competing with a scrolling board is how you end
// up filing "asdf" on a shared Jira board.
//
// On the two green-ish affordances: `+ Add` on a card is green (a commit - that task
// is going in today), while ＋ New task is violet-tinted because it opens the
// AI-drafting surface. Different acts, different hues. See planStyles' header.

import { useState } from 'react'
import { Droppable } from '@hello-pangea/dnd'
import { DraggableCard } from '@/components/plan/DraggableCard'
import { TaskComposer } from '@/components/plan/TaskComposer'
import type { CardTask } from '@/components/plan/TaskCard'
import { FOCUS } from '@/components/plan/planStyles'
import { MAX_PLAN_TASKS } from '@/lib/api-types'
import type { Tracker } from '@/lib/integrations'

export function PlanBoardColumn({
  day, board, trackers, editable, planFull, search, sortMode,
  onSearch, onSort, onAdd, onOpen, onCreated,
}: {
  day: string
  board: CardTask[]
  /** Connected trackers - empty means the composer offers no sync choice. */
  trackers: Tracker[]
  editable: boolean
  /** The day is already at MAX_PLAN_TASKS. Creating a task ALWAYS adds it to the
   *  plan, so with no room there is nowhere for it to land - and creating on a
   *  tracker files a real ticket on a real board. Blocking the composer here is
   *  what stops that outward-facing act from happening for nothing (the daemon
   *  refuses too, before it files anything - see plan_tasks::create). */
  planFull: boolean
  search: string
  sortMode: 'top' | 'due' | 'az'
  onSearch: (q: string) => void
  onSort: (m: 'top' | 'due' | 'az') => void
  onAdd: (task: CardTask) => void
  onOpen: (task: CardTask) => void
  /** A task was created and is already in the plan - refresh the board. */
  onCreated: () => void
}) {
  const [composing, setComposing] = useState(false)

  return (
    <div className="rounded-xl flex flex-col min-h-0 bg-card p-4" style={{ border: '1px solid var(--t-card-border)' }}>
      {composing ? (
        <TaskComposer
          day={day} trackers={trackers}
          onCancel={() => setComposing(false)}
          onDone={() => { setComposing(false); onCreated() }}
        />
      ) : (
        <>
          <div className="shrink-0">
            <div className="flex items-center justify-between gap-2 mb-3">
              <p className="mt-label" style={{ color: 'var(--t-faint)' }}>
                Your tasks{search ? ` · ${board.length} match${board.length === 1 ? '' : 'es'}` : ` · ${board.length}`}
              </p>
              {/* NOT gated on `editable`. The lock exists so a confirmed plan isn't
                  silently reshuffled - it is not a reason to refuse to let someone
                  write down new work. Authoring a task is its own act; it writes
                  server-side and the plan re-derives from the server, so a locked
                  plan simply gains the task rather than being edited behind your
                  back. Hiding this behind Edit plan made it undiscoverable. */}
              <button onClick={() => setComposing(true)} disabled={planFull}
                title={planFull ? `Today already has ${MAX_PLAN_TASKS} tasks - remove one to add another` : undefined}
                className={`shrink-0 mt-chip px-3 py-1.5 rounded-md transition-opacity hover:opacity-80 ${FOCUS}`}
                style={{
                  background: 'color-mix(in srgb, var(--color-state-proposal) 14%, transparent)',
                  color: 'var(--color-state-proposal)', fontWeight: 700,
                  opacity: planFull ? 0.45 : 1, cursor: planFull ? 'default' : 'pointer',
                }}>
                ＋ New task
              </button>
            </div>
            <div className="flex items-center gap-2 mb-3">
              <input
                value={search} onChange={e => onSearch(e.target.value)} aria-label="Search your tasks"
                placeholder="Search tasks…"
                className={`flex-1 mt-body-sm px-3 py-2 rounded-md ${FOCUS}`}
                style={{ background: 'var(--t-input)', color: 'var(--t-title)', border: '1px solid var(--t-input-border)' }}
              />
              <div role="radiogroup" aria-label="Sort tasks" className="flex rounded-md overflow-hidden" style={{ border: '1px solid var(--t-ctrl-border)' }}>
                {(['top', 'due', 'az'] as const).map(m => (
                  <button key={m} role="radio" aria-checked={sortMode === m} onClick={() => onSort(m)}
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
                className="space-y-2 flex-1 min-h-0 overflow-y-auto nice-scroll rounded-xl transition-colors p-1 pr-1.5"
                style={{ outline: snapshot.isDraggingOver ? '1.5px dashed var(--t-hair)' : '1.5px dashed transparent', outlineOffset: 2 }}>
                {board.map((t, i) => (
                  <DraggableCard key={t.key} task={t} index={i} detail draggable={editable} onOpen={() => onOpen(t)}
                    trail={editable
                      ? (
                        <button onClick={() => onAdd(t)} aria-label={`Add ${t.key} to today`}
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
        </>
      )}
    </div>
  )
}
