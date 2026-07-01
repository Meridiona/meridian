//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Trivial switch between the Overview panel (no hour selected) and the
// Hour-detail panel (an hour is selected).

'use client'

import { OverviewPanel } from './OverviewPanel'
import { HourDetailPanel } from './HourDetailPanel'
import type { TimelineData } from './useTimelineData'
import type { ActiveModal } from './MeridianTimelineShell'

export function RightPanel({ data, selectedHour, selectedCardKey, onSelectHour, onOpen, onOpenTask }: {
  data: TimelineData
  selectedHour: number | null
  selectedCardKey: string | null
  onSelectHour: (hour: number | null) => void
  onOpen: (modal: ActiveModal) => void
  onOpenTask: (key: string, title?: string) => void
}) {
  if (selectedHour === null) return <OverviewPanel data={data} onOpen={onOpen} onOpenTask={onOpenTask} />
  return (
    <HourDetailPanel
      hour={selectedHour}
      selectedCardKey={selectedCardKey}
      onBack={() => onSelectHour(null)}
      data={data}
    />
  )
}
