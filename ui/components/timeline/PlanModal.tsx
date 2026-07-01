//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Daily-plan modal — wraps the existing PlanView unchanged.

'use client'

import PlanView from '@/components/views/PlanView'
import { ModalShell } from './ModalShell'

export function PlanModal({ onClose }: { onClose: () => void }) {
  return (
    <ModalShell title="Daily plan" onClose={onClose} maxWidth={860}>
      <PlanView />
    </ModalShell>
  )
}
