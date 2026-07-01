//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Board-cleanup modal — wraps the existing CleanupView unchanged (it fetches
// get_tasks + get_integrations itself; the shell's cleanupIssueCount only
// decides whether to surface the entry point, so no double-fetch coordination
// is needed here).

'use client'

import CleanupView from '@/components/views/CleanupView'
import { ModalShell } from './ModalShell'

export function CleanupModal({ onClose }: { onClose: () => void }) {
  return (
    <ModalShell title="Board clean-up" onClose={onClose} maxWidth={860}>
      <CleanupView />
    </ModalShell>
  )
}
