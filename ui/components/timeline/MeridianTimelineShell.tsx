//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The entire one-pager Meridian Timeline app. ui/app/page.tsx renders this
// directly (no DashboardShell/Sidebar/CommandBar). Owns the selected day, the
// selected hour (Overview ↔ Hour-detail), and which modal is open; calls
// useTimelineData ONCE at the top and threads it down. Applies the persisted
// theme on mount. Fluid app-like layout (h-[100svh], own inner scroll regions)
// — the mock's fake window chrome is intentionally dropped (Tauri provides it).

'use client'

import { useEffect, useState } from 'react'
import { load } from '@/lib/bridge'
import type { RuntimeSettings } from '@/lib/settings'
import { applyTheme } from '@/lib/theme'
import HealthBanner from '@/components/HealthBanner'
import MustFixBanner from '@/components/MustFixBanner'
import { useTimelineData } from './useTimelineData'
import { dayString, shiftDay, isPending } from './types'
import { Toolbar } from './Toolbar'
import { TimelineColumn } from './TimelineColumn'
import { RightPanel } from './RightPanel'
import { FloatingDraftsPill } from './FloatingDraftsPill'
import { ReviewModal } from './ReviewModal'
import { CleanupModal } from './CleanupModal'
import { SettingsModal } from './SettingsModal'
import { PlanModal } from './PlanModal'
import { TasksModal } from './TasksModal'
import { TaskDetailDialog } from './TaskDetailDialog'

export type ActiveModal = 'review' | 'cleanup' | 'settings' | 'plan' | 'tasks' | null

export default function MeridianTimelineShell() {
  const [day, setDay] = useState<string>(dayString(0))
  const [selectedHour, setSelectedHour] = useState<number | null>(null)
  // Set only when a specific worklog card (not the hour row itself) was
  // clicked — narrows the Hour-detail panel to that one card instead of every
  // ticket in the hour, and suppresses the row-level highlight (the card
  // itself gets the "popped forward" treatment instead).
  const [selectedCardKey, setSelectedCardKey] = useState<string | null>(null)
  const [activeModal, setActiveModal] = useState<ActiveModal>(null)
  // The ticket detail dialog is a separate, stackable layer (not part of
  // ActiveModal) — it can open on top of the Tasks/Plan modals or straight
  // from the timeline/Overview panel.
  const [openTask, setOpenTask] = useState<{ key: string; title?: string } | null>(null)

  const data = useTimelineData(day)
  const { items, isSolo, connectedProviderName, isToday } = data
  const pendingCount = items.filter(isPending).length

  // Apply the persisted theme on mount (before any round-trip resolves elsewhere).
  useEffect(() => {
    load<RuntimeSettings>('/api/settings', 'get_settings')
      .then(s => applyTheme(s.theme))
      .catch(() => {})
  }, [])

  // NoticeBar lives at the root layout, outside this tree, so its "Fix in
  // Tasks" action reaches the Tasks modal via a window event instead of props.
  useEffect(() => {
    const open = () => setActiveModal('tasks')
    window.addEventListener('meridian:open-tasks', open)
    return () => window.removeEventListener('meridian:open-tasks', open)
  }, [])

  // Changing day resets the selected hour (its detail no longer applies).
  function shift(delta: number) {
    setSelectedHour(null)
    setSelectedCardKey(null)
    setDay(d => shiftDay(d, delta))
  }

  // Row-level selection (Quiet/solo rows, or blank space in a row) — shows
  // every ticket in the hour and clears any single-card selection.
  function selectHour(hour: number | null) {
    setSelectedHour(hour)
    setSelectedCardKey(null)
  }

  // Card-level selection — narrows Hour-detail to just this one card.
  function selectCard(hour: number, cardKey: string) {
    setSelectedHour(hour)
    setSelectedCardKey(cardKey)
  }

  return (
    <div className="relative h-[100svh] overflow-hidden flex flex-col" style={{ background: 'var(--win-bg)' }}>
      <HealthBanner />
      <MustFixBanner
        onOpenCleanup={() => setActiveModal('cleanup')}
        hidden={activeModal === 'cleanup'}
      />

      <Toolbar
        day={day}
        isToday={isToday}
        onShiftDay={shift}
        isSolo={isSolo}
        connectedProviderName={connectedProviderName}
        onOpenSettings={() => setActiveModal('settings')}
      />

      <div className="flex flex-1 min-h-0">
        <div className="relative flex-1 min-w-0 min-h-0 flex flex-col">
          <TimelineColumn
            hourBuckets={data.hourBuckets}
            isSolo={isSolo}
            today={data.today}
            selectedHour={selectedHour}
            selectedCardKey={selectedCardKey}
            onSelectHour={selectHour}
            onSelectCard={selectCard}
            isToday={isToday}
            day={day}
          />

          {!isSolo && (
            <FloatingDraftsPill count={pendingCount} onClick={() => setActiveModal('review')} />
          )}
        </div>
        <div className="shrink-0 border-l min-h-0" style={{ width: 388, borderColor: 'var(--t-hair)', background: 'var(--t-panel)' }}>
          <RightPanel
            data={data}
            selectedHour={selectedHour}
            selectedCardKey={selectedCardKey}
            onSelectHour={selectHour}
            onOpen={setActiveModal}
            onOpenTask={(key, title) => setOpenTask({ key, title })}
          />
        </div>
      </div>

      {activeModal === 'review' && (
        <ReviewModal items={items} actions={data.actions} onClose={() => setActiveModal(null)} />
      )}
      {activeModal === 'cleanup' && <CleanupModal onClose={() => setActiveModal(null)} />}
      {activeModal === 'settings' && <SettingsModal onClose={() => setActiveModal(null)} />}
      {activeModal === 'plan' && <PlanModal onClose={() => setActiveModal(null)} />}
      {activeModal === 'tasks' && (
        <TasksModal onClose={() => setActiveModal(null)} onOpenTask={(key, title) => setOpenTask({ key, title })} />
      )}
      {openTask && (
        <TaskDetailDialog taskKey={openTask.key} fallbackTitle={openTask.title} onClose={() => setOpenTask(null)} />
      )}
    </div>
  )
}
