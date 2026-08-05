//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// The left "Today" column: the ordered focus list plus the footer that commits it.
// Extracted verbatim from PlanView, which kept only the shell and the mode algebra.
//
// Props-only and stateless on purpose — PlanView owns `today` because onDragEnd
// mutates it across BOTH columns, so the list can't live in either one of them.
//
// The footer used to render a four-state mode algebra (proposed / editing /
// confirmed-locked / skipped) around a Confirm button. There is no such algebra
// any more: every change to the list saves itself in `PlanView.commit`, so the
// only distinction left is skipped vs not, and the footer's job shrank to the one
// control that still represents a decision - opting out of planning today.

import { Droppable } from '@hello-pangea/dnd'
import { DraggableCard } from '@/components/plan/DraggableCard'
import type { CardTask } from '@/components/plan/TaskCard'
import { FOCUS, PRIMARY_BTN } from '@/components/plan/planStyles'
import { MAX_PLAN_TASKS } from '@/lib/api-types'

export function PlanTodayColumn({
  today, editable, skipped, onRemove, onOpen, onSkip, onReopen,
}: {
  today: CardTask[]
  /** The dev can drag / add / remove right now - true unless the day is skipped. */
  editable: boolean
  skipped: boolean
  onRemove: (key: string) => void
  onOpen: (task: CardTask) => void
  onSkip: () => void
  onReopen: () => void
}) {
  return (
    <div className="rounded-xl flex flex-col min-h-0 bg-card" style={{ border: '1px solid var(--t-card-border)' }}>
      <div className="shrink-0 px-4 pt-4">
        <div className="flex items-center justify-between mb-1">
          {/* A real column heading, matching PlanBoardColumn's. "Today" and "Your
              tasks" are the only structural labels on this screen - they say which
              half is your day and which half is your board - and as `.mt-label`
              micro-caps they read as field labels for the controls under them. */}
          <p style={{ font: '800 16px var(--font-sans)', letterSpacing: '-.012em', color: 'var(--t-title)' }}>
            Today
            <span className="mt-body-sm" style={{ color: 'var(--t-faint)', fontWeight: 500, marginLeft: 7 }}>{today.length}</span>
          </p>
          {/* The "Suggested" chip is gone with the draft state it labelled. Every
              card in this column is now something the user put there and the app
              has saved; calling that a suggestion would be untrue of all of it. */}
          {!skipped && today.length > 0 && (
            <span className="mt-chip px-1.5 py-0.5 rounded"
              style={{ color: 'var(--color-state-approved)', background: 'color-mix(in srgb, var(--color-state-approved) 12%, transparent)' }}>
              Saved
            </span>
          )}
        </div>
        <p className="mt-body-sm mb-2" style={{ color: 'var(--t-faint)' }}>
          {skipped
            ? 'You skipped planning today.'
            : 'Drag to reorder, drag a card out to remove, or add from your board - every change saves itself.'}
        </p>
        {/* Two rungs: a nag at 5, a wall at MAX_PLAN_TASKS. The wall is mirrored in
            Rust (plan::check_plan_size) - this is just the faster of the two. */}
        {editable && today.length >= MAX_PLAN_TASKS && (
          <p className="mt-body-sm mb-2 flex items-center gap-1.5" style={{ color: 'var(--color-state-pending)' }}>
            <span>⚠</span> That&apos;s the {MAX_PLAN_TASKS}-task limit for a day. Remove one to add another.
          </p>
        )}
        {editable && today.length > 5 && today.length < MAX_PLAN_TASKS && (
          <p className="mt-body-sm mb-2 flex items-center gap-1.5" style={{ color: 'var(--color-state-pending)' }}>
            <span>⚠</span> That&apos;s a full plate — most focused days land on 1–3 tasks.
          </p>
        )}
      </div>

      <Droppable droppableId="today">
        {(provided, snapshot) => (
          <div ref={provided.innerRef} {...provided.droppableProps}
            // data-tour: the walkthrough spotlights this as the drop TARGET while it
            // asks the user to drag today's work across. It is the drop zone itself,
            // not the header, so the ring lands on the area the cursor must reach.
            data-tour="plan-today"
            className="rounded-xl transition-colors flex-1 min-h-0 overflow-y-auto nice-scroll mx-4 mb-2 p-2 space-y-2"
            style={{
              background: snapshot.isDraggingOver ? 'color-mix(in srgb, var(--color-state-proposal) 10%, transparent)' : 'var(--t-box)',
              outline: snapshot.isDraggingOver ? '1.5px dashed var(--color-state-proposal)' : '1.5px dashed transparent', outlineOffset: 2,
            }}>
            {today.map((t, i) => (
              <DraggableCard key={t.key} task={t} index={i} draggable={editable} onOpen={() => onOpen(t)}
                trail={
                  <div className="flex items-center gap-1 shrink-0">
                    <span aria-hidden className="mt-mono-sm text-[11px] mr-1" style={{ color: 'var(--t-faint-2)' }}>{i + 1}</span>
                    {editable && (
                      <button onClick={() => onRemove(t.key)} aria-label={`Remove ${t.key} from today`}
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

      {/* One control, because one decision is left. Confirm / Save changes /
          Cancel / Edit plan all existed to gate a write that now happens as the
          user acts, and a button that agrees with something you already did is a
          step whose only real effect is letting you lose the work by not pressing
          it. Skipping the day is genuinely a choice, so it stays. */}
      <div className="shrink-0 px-4 py-3.5 border-t flex items-center gap-3" style={{ borderColor: 'var(--t-hair)' }}>
        {skipped ? (
          <button onClick={onReopen}
            className={`${PRIMARY_BTN} ${FOCUS}`} style={{ background: 'var(--color-state-approved)', color: '#fff' }}>
            Plan today →
          </button>
        ) : (
          <>
            <span className="mt-body-sm" style={{ color: 'var(--t-faint-2)' }}>These lead today’s task matching.</span>
            <button onClick={onSkip} className={`mt-body-sm px-2 py-1.5 rounded-md ml-auto ${FOCUS}`} style={{ color: 'var(--t-faint-2)' }}>
              Skip today
            </button>
          </>
        )}
      </div>
    </div>
  )
}
