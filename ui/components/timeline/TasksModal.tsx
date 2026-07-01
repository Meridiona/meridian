//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Tasks modal — wraps the existing TasksView unchanged. Opened from the
// Overview panel's Tasks entry point (Tasks is a modal, not a route).

'use client'

import TasksView from '@/components/views/TasksView'
import { ModalShell } from './ModalShell'

export function TasksModal({ onClose }: { onClose: () => void }) {
  return (
    <ModalShell title="Tasks" onClose={onClose} maxWidth={980}>
      <TasksView />
    </ModalShell>
  )
}
