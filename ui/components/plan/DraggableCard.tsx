//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// One draggable task row, shared by both plan columns. Extracted from PlanView so the
// columns could be split out without duplicating it.
//
// When `draggable` is false (a confirmed, locked plan) the grip disappears and the row
// can't be picked up — the lock is real, not just visual.

import { Draggable, type DraggableProvided, type DraggableStateSnapshot } from '@hello-pangea/dnd'
import { GripHandle } from '@/components/plan/parts'
import { TaskCardBody, type CardTask } from '@/components/plan/TaskCard'
import { FOCUS } from '@/components/plan/planStyles'

export function DraggableCard({
  task, index, trail, detail = false, onOpen, draggable = true,
}: {
  task: CardTask
  index: number
  /** Trailing controls (remove / + Add). */
  trail?: React.ReactNode
  /** Board column: also show the description excerpt, priority and points. */
  detail?: boolean
  onOpen?: () => void
  draggable?: boolean
}) {
  return (
    <Draggable draggableId={task.key} index={index} isDragDisabled={!draggable}>
      {(provided: DraggableProvided, snapshot: DraggableStateSnapshot) => (
        <div ref={provided.innerRef} {...provided.draggableProps}
          style={{
            ...provided.draggableProps.style,
            borderColor: 'var(--t-card-border)',
            background: 'var(--t-card)',
            // The one lifted element in this list, so it's the one that gets a shadow.
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
