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
import { isResumePending } from '@/components/plan/useTaskComposer'
import type { CardTask } from '@/components/plan/TaskCard'
import { FOCUS } from '@/components/plan/planStyles'
import { MAX_PLAN_TASKS } from '@/lib/api-types'
import type { Tracker } from '@/lib/integrations'

export function PlanBoardColumn({
  day, board, trackers, editable, planFull, pulling = false, search, sortMode,
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
  /** A tracker sync is in flight, so an empty list means "not here YET" rather
   *  than "nothing to show". Only the empty message reads it - the column is
   *  otherwise fully usable while tickets arrive. */
  pulling?: boolean
  search: string
  sortMode: 'top' | 'due' | 'az'
  onSearch: (q: string) => void
  onSort: (m: 'top' | 'due' | 'az') => void
  onAdd: (task: CardTask) => void
  onOpen: (task: CardTask) => void
  /** A task was created and is already in the plan - refresh the board. */
  onCreated: () => void
}) {
  // OPEN ON A RESUME, not just on a click. Pressing "Draft with AI" with no provider
  // connected sends the user to Settings, which takes the shell's single modal slot and
  // unmounts this column - taking `composing` with it. The note itself survived (the
  // composer's store is a module store for exactly this), but the user was handed back the
  // BOARD LIST and had to press ＋ New task again to find it, which reads as having lost
  // the work whether or not it did. During the walkthrough it also stranded the tour: the
  // next beat points at fields that only exist inside the composer.
  //
  // `isResumePending` PEEKS. The flag is `TaskComposer`'s to spend - its mount effect reads
  // it to know it must keep the note instead of resetting - so consuming it here would
  // reopen the composer onto a box this call had just emptied.
  const [composing, setComposing] = useState(isResumePending)

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
              {/* A real column heading, matching PlanTodayColumn's. These two are
                  the only structural labels on the screen - which half is your
                  board and which half is your day - and as `.mt-label` micro-caps
                  they read as field labels for the controls under them instead. */}
              <p style={{ font: '800 16px var(--font-sans)', letterSpacing: '-.012em', color: 'var(--t-title)' }}>
                Your tasks
                <span className="mt-body-sm" style={{ color: 'var(--t-faint)', fontWeight: 500, marginLeft: 7 }}>
                  {search ? `${board.length} match${board.length === 1 ? '' : 'es'}` : board.length}
                </span>
              </p>
              {/* NOT gated on `editable`. The lock exists so a confirmed plan isn't
                  silently reshuffled - it is not a reason to refuse to let someone
                  write down new work. Authoring a task is its own act; it writes
                  server-side and the plan re-derives from the server, so a locked
                  plan simply gains the task rather than being edited behind your
                  back. Hiding this behind Edit plan made it undiscoverable. */}
              {/* SOLID, not tinted. This is the only way to author a task in the
                  planner and it was a 14%-alpha chip the size of a sort toggle,
                  sitting one row above three more grey controls - it read as the
                  fourth filter rather than as the primary action of the column, and
                  the walkthrough had to point at it because nobody found it. Filled
                  accent + white label + the button's own shadow is the same weight
                  class as "Add to today", which is correct: they are the two things
                  on this screen that CREATE something. Violet rather than the
                  commit-green of `+ Add`, because this opens the AI drafting
                  surface - see this file's header on the two hues. */}
              {/* data-tour: an inert hook for the walkthrough's "you can write a task
                  here without opening Jira" beat. See components/tutorial/script.ts. */}
              <button data-tour="plan-new-task" onClick={() => setComposing(true)} disabled={planFull}
                title={planFull ? `Today already has ${MAX_PLAN_TASKS} tasks - remove one to add another` : undefined}
                className={`shrink-0 mt-body-sm inline-flex items-center gap-1.5 px-3.5 py-2 rounded-lg transition-opacity hover:opacity-90 ${FOCUS}`}
                style={{
                  background: 'var(--btn-primary-bg)', color: '#fff', fontWeight: 700,
                  boxShadow: planFull ? 'none' : '0 8px 22px -10px var(--t-accent)',
                  opacity: planFull ? 0.45 : 1, cursor: planFull ? 'default' : 'pointer',
                }}>
                <span aria-hidden style={{ fontSize: 15, lineHeight: 1, marginTop: -1 }}>＋</span> New task
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
                // data-tour: the walkthrough's demonstration drag picks its source
                // card from in here (`[data-tour="plan-board"] [data-plan-card]`) -
                // it cannot name a ticket, since the board is whatever the user's
                // own tracker returned.
                data-tour="plan-board"
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
                    {search
                      ? 'No tasks match your search.'
                      // "Everything is in today's plan" is a confident claim, and it
                      // was being made about a board that had simply not finished
                      // loading - the one moment it is most likely to be false.
                      : pulling
                        ? 'Pulling in your tickets…'
                        : 'Everything is in today’s plan. Drag a card here to remove it.'}
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
