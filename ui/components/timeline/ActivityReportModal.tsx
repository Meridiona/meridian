//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Big-screen dialog for an hour's FULL activity report — opened from
// HourDetailPanel's condensed summary (TLDR + Core Tasks names only) via a
// "View full report" button. Reuses ModalShell's overlay/backdrop/Escape
// chrome; wide (860px) so the report's markdown reads comfortably at full
// type scale (ActivityReport, non-compact).

'use client'

import { ModalShell } from './ModalShell'
import { ActivityReport } from './ActivityReport'
import { hourLabel } from './timelineLayout'

export function ActivityReportModal({ hour, report, onClose }: {
  hour: number
  report: string
  onClose: () => void
}) {
  return (
    <ModalShell title={`${hourLabel(hour)} · Activity report`} onClose={onClose} maxWidth={860}>
      <div className="rounded-xl p-6 bg-box">
        <ActivityReport report={report} />
      </div>
    </ModalShell>
  )
}
