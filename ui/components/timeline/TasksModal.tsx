//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Tasks modal — wraps the restyled TasksPanel. Opened from the Overview
// panel's Tasks entry point (Tasks is a modal, not a route).

'use client'

import { TasksPanel } from './TasksPanel'
import { ModalShell } from './ModalShell'

export function TasksModal({ onClose, onOpenTask }: {
  onClose: () => void
  onOpenTask: (key: string, title?: string) => void
}) {
  return (
    <ModalShell title="Tasks" onClose={onClose} maxWidth={980}>
      <TasksPanel onOpenTask={onOpenTask} />
    </ModalShell>
  )
}
