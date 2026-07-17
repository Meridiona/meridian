//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// The left "Today" column: the ordered focus list plus the footer that commits it.
// Extracted verbatim from PlanView, which kept only the shell and the mode algebra.
//
// Props-only and stateless on purpose — PlanView owns `today` because onDragEnd
// mutates it across BOTH columns, so the list can't live in either one of them.
//
// The footer's four states (proposed / skipped / editing / confirmed-locked) are the
// mode algebra rendered: PlanView decides which one is live, this file only draws it.

import { Droppable } from '@hello-pangea/dnd'
import { DraggableCard } from '@/components/plan/DraggableCard'
import type { CardTask } from '@/components/plan/TaskCard'
import { FOCUS, PRIMARY_BTN, GHOST_BTN } from '@/components/plan/planStyles'
import { MAX_PLAN_TASKS } from '@/lib/api-types'

export function PlanTodayColumn({
  today, editable, proposed, editing, confirmedMode, skipped,
  onRemove, onOpen, onConfirm, onSkip, onReopen, onEdit, onSave, onCancel,
}: {
  today: CardTask[]
  /** The dev can drag / add / remove right now. */
  editable: boolean
  /** The pre-confirm draft. */
  proposed: boolean
  /** An unlocked "Edit plan" session over a confirmed plan. */
  editing: boolean
  confirmedMode: boolean
  skipped: boolean
  onRemove: (key: string) => void
  onOpen: (task: CardTask) => void
  onConfirm: () => void
  onSkip: () => void
  onReopen: () => void
  onEdit: () => void
  onSave: () => void
  onCancel: () => void
}) {
  return (
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

      <div className="shrink-0 px-4 py-3.5 border-t flex items-center gap-3" style={{ borderColor: 'var(--t-hair)' }}>
        {proposed ? (
          <>
            <button onClick={onConfirm}
              className={`${PRIMARY_BTN} ${FOCUS}`} style={{ background: 'var(--color-state-approved)', color: '#fff' }}>
              Confirm {today.length > 0 ? `${today.length} task${today.length === 1 ? '' : 's'}` : 'plan'} →
            </button>
            <button onClick={onSkip} className={`mt-body-sm px-2 py-1.5 rounded-md ml-auto ${FOCUS}`} style={{ color: 'var(--t-faint-2)' }}>
              Skip today
            </button>
          </>
        ) : skipped ? (
          <button onClick={onReopen}
            className={`${PRIMARY_BTN} ${FOCUS}`} style={{ background: 'var(--color-state-approved)', color: '#fff' }}>
            Plan today →
          </button>
        ) : editing ? (
          <>
            <button onClick={onSave} className={`${PRIMARY_BTN} ${FOCUS}`} style={{ background: 'var(--color-state-approved)', color: '#fff' }}>
              Save changes
            </button>
            <button onClick={onCancel} className={`mt-body-sm px-2 py-1.5 rounded-md ${FOCUS}`} style={{ color: 'var(--t-faint-2)' }}>
              Cancel
            </button>
          </>
        ) : (
          <>
            <button onClick={onEdit} className={`${GHOST_BTN} ${FOCUS}`}
              style={{ border: '1px solid var(--t-ctrl-border)', color: 'var(--t-muted)' }}>
              Edit plan
            </button>
            <span className="mt-body-sm ml-auto" style={{ color: 'var(--t-faint-2)' }}>These lead today’s task matching.</span>
          </>
        )}
      </div>
    </div>
  )
}
