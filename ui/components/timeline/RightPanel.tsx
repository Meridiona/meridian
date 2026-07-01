//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Trivial switch between the Overview panel (no hour selected) and the
// Hour-detail panel (an hour is selected).

'use client'

import { OverviewPanel } from './OverviewPanel'
import { HourDetailPanel } from './HourDetailPanel'
import type { TimelineData } from './useTimelineData'
import type { ActiveModal } from './MeridianTimelineShell'

export function RightPanel({ data, selectedHour, onSelectHour, onOpen }: {
  data: TimelineData
  selectedHour: number | null
  onSelectHour: (hour: number | null) => void
  onOpen: (modal: ActiveModal) => void
}) {
  if (selectedHour === null) return <OverviewPanel data={data} onOpen={onOpen} />
  return <HourDetailPanel hour={selectedHour} onBack={() => onSelectHour(null)} data={data} />
}
