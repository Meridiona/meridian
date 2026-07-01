//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Daily-plan modal — wraps PlanView, themed with the mt-* timeline tokens.
// `scrollInside` hands the body a bounded flex box so PlanView's two columns
// scroll independently instead of the whole modal scrolling.

'use client'

import PlanView from '@/components/views/PlanView'
import { ModalShell } from './ModalShell'

export function PlanModal({ onClose }: { onClose: () => void }) {
  return (
    <ModalShell title="Daily plan" onClose={onClose} maxWidth={920} scrollInside>
      <PlanView />
    </ModalShell>
  )
}
