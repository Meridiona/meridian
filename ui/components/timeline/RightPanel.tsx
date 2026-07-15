//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Trivial switch between the Overview panel (no hour selected) and the
// Hour-detail panel (an hour is selected).

'use client'

import { OverviewPanel } from './OverviewPanel'
import { HourDetailPanel } from './HourDetailPanel'
import { DayTaskDetailPanel, type DayTaskDetail } from './DayTaskDetailPanel'
import type { TimelineData } from './useTimelineData'
import type { ActiveModal } from './MeridianTimelineShell'
import type { SettingsSection } from './settings/types'

export function RightPanel({ data, selectedHour, selectedCardKey, dayTaskDetail, onCloseDayTask, onSelectHour, onOpen, onOpenTask, onEditWorklog, onOpenSettings }: {
  data: TimelineData
  selectedHour: number | null
  selectedCardKey: string | null
  // A day-task selected in the timeline column — when set, its detail replaces
  // the overview here (same swap the hour detail uses).
  dayTaskDetail: DayTaskDetail | null
  onCloseDayTask: () => void
  onSelectHour: (hour: number | null) => void
  onOpen: (modal: ActiveModal) => void
  onOpenTask: (key: string, title?: string) => void
  // Edit an approved/posted card — opens the same Review dialog drafts use,
  // scoped to this one ticket (see MeridianTimelineShell's openReview).
  onEditWorklog: (cardKey: string) => void
  // Deep-link into Settings (e.g. 'integrations') — the solo-mode "Connect a
  // tracker" CTAs in OverviewPanel/HourDetailPanel use this.
  onOpenSettings: (section?: SettingsSection) => void
}) {
  if (dayTaskDetail) {
    return <DayTaskDetailPanel detail={dayTaskDetail} onClose={onCloseDayTask} />
  }
  if (selectedHour === null) {
    return <OverviewPanel data={data} onOpen={onOpen} onOpenTask={onOpenTask} onOpenSettings={onOpenSettings} />
  }
  return (
    <HourDetailPanel
      hour={selectedHour}
      selectedCardKey={selectedCardKey}
      onBack={() => onSelectHour(null)}
      data={data}
      onEditWorklog={onEditWorklog}
      onOpenSettings={onOpenSettings}
    />
  )
}
