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

export function RightPanel({ data, selectedHour, selectedCardKey, dayTaskDetail, onCloseDayTask, onDayTaskCorrected, onSelectHour, onOpen, onOpenTask, onEditWorklog, onOpenSettings }: {
  data: TimelineData
  selectedHour: number | null
  selectedCardKey: string | null
  // A day-task selected in the timeline column — when set, its detail replaces
  // the overview here (same swap the hour detail uses).
  dayTaskDetail: DayTaskDetail | null
  onCloseDayTask: () => void
  // A day-task was dismissed or merged — the shell clears selection and reloads
  // the timeline column.
  onDayTaskCorrected: () => void
  onSelectHour: (hour: number | null) => void
  onOpen: (modal: ActiveModal) => void
  // `editable` (only ever passed by OverviewPanel's Today's-focus checklist) is
  // false for a past day's focus item — you can look back at what a prior day's
  // plan was, but not rewrite it after the fact.
  onOpenTask: (key: string, title?: string, editable?: boolean) => void
  // Edit an approved/posted card — opens the same Review dialog drafts use,
  // scoped to this one ticket (see MeridianTimelineShell's openReview).
  onEditWorklog: (cardKey: string) => void
  // Deep-link into Settings (e.g. 'integrations') — the solo-mode "Connect a
  // tracker" CTAs in OverviewPanel/HourDetailPanel use this.
  onOpenSettings: (section?: SettingsSection) => void
}) {
  if (dayTaskDetail) {
    return <DayTaskDetailPanel detail={dayTaskDetail} onClose={onCloseDayTask}
      onCorrected={onDayTaskCorrected} onOpenSettings={onOpenSettings} onOpenTask={onOpenTask} />
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
