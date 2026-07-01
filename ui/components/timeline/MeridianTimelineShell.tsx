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

export type ActiveModal = 'review' | 'cleanup' | 'settings' | 'plan' | 'tasks' | null

export default function MeridianTimelineShell() {
  const [day, setDay] = useState<string>(dayString(0))
  const [selectedHour, setSelectedHour] = useState<number | null>(null)
  const [activeModal, setActiveModal] = useState<ActiveModal>(null)

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
    setDay(d => shiftDay(d, delta))
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

      <div className="relative flex flex-1 min-h-0">
        <TimelineColumn
          hourBuckets={data.hourBuckets}
          isSolo={isSolo}
          today={data.today}
          selectedHour={selectedHour}
          onSelectHour={setSelectedHour}
          isToday={isToday}
        />
        <div className="shrink-0 border-l min-h-0" style={{ width: 388, borderColor: 'var(--t-hair)', background: 'var(--t-panel)' }}>
          <RightPanel
            data={data}
            selectedHour={selectedHour}
            onSelectHour={setSelectedHour}
            onOpen={setActiveModal}
          />
        </div>

        {!isSolo && (
          <FloatingDraftsPill count={pendingCount} onClick={() => setActiveModal('review')} />
        )}
      </div>

      {activeModal === 'review' && (
        <ReviewModal items={items} actions={data.actions} onClose={() => setActiveModal(null)} />
      )}
      {activeModal === 'cleanup' && <CleanupModal onClose={() => setActiveModal(null)} />}
      {activeModal === 'settings' && <SettingsModal onClose={() => setActiveModal(null)} />}
      {activeModal === 'plan' && <PlanModal onClose={() => setActiveModal(null)} />}
      {activeModal === 'tasks' && <TasksModal onClose={() => setActiveModal(null)} />}
    </div>
  )
}
