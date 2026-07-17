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
import { invoke, load, subscribe } from '@/lib/bridge'
import type { RuntimeSettings } from '@/lib/settings'
import { applyTheme } from '@/lib/theme'
import HealthBanner from '@/components/HealthBanner'
import { useTimelineData } from './useTimelineData'
import { dayString, shiftDay, isPending } from './types'
import { Toolbar } from './Toolbar'
import { DayTaskColumn } from './DayTaskColumn'
import type { DayTaskDetail } from './DayTaskDetailPanel'
import { RightPanel } from './RightPanel'
import { FloatingDraftsPill } from './FloatingDraftsPill'
import { ReviewModal } from './ReviewModal'
import { CleanupModal } from './CleanupModal'
import { SettingsModal } from './SettingsModal'
import { PlanModal } from './PlanModal'
import { TasksModal } from './TasksModal'
import { TaskDetailDialog } from './TaskDetailDialog'
import { ReportModal } from './ReportModal'
import { LlmLabModal } from './llmlab/LlmLabModal'
import { WhatsNewModal } from './WhatsNewModal'
import type { AppInfo } from '@/lib/api-types'
import type { SettingsSection } from './settings/types'

export type ActiveModal =
  | 'review' | 'cleanup' | 'settings' | 'plan' | 'tasks' | 'report' | 'llmlab' | 'whats-new' | null

export default function MeridianTimelineShell() {
  const [day, setDay] = useState<string>(dayString(0))
  const [selectedHour, setSelectedHour] = useState<number | null>(null)
  // A day-task card clicked in the timeline column — its detail replaces "Today
  // at a glance" in the right panel (same swap the hour/approved-card detail
  // uses), so the timeline keeps the clicked card highlighted and the rest dulled.
  const [selectedDayTask, setSelectedDayTask] = useState<DayTaskDetail | null>(null)
  // Set only when a specific worklog card (not the hour row itself) was
  // clicked — narrows the Hour-detail panel to that one card instead of every
  // ticket in the hour, and suppresses the row-level highlight (the card
  // itself gets the "popped forward" treatment instead).
  const [selectedCardKey, setSelectedCardKey] = useState<string | null>(null)
  const [activeModal, setActiveModal] = useState<ActiveModal>(null)
  // Set when a still-drafted card was clicked directly (as opposed to the
  // floating pill / nav, which review the whole pending queue) — scopes the
  // Review dialog to just that one ticket instead of the full queue.
  const [reviewFocusKey, setReviewFocusKey] = useState<string | null>(null)
  // Which Settings tab to land on when the modal opens — set by callers that
  // deep-link (e.g. the nav pill's "Integrations" item); undefined defaults
  // to Settings' own DEFAULT_SETTINGS_SECTION.
  const [settingsSection, setSettingsSection] = useState<SettingsSection | undefined>(undefined)
  // The ticket detail dialog is a separate, stackable layer (not part of
  // ActiveModal) — it can open on top of the Tasks/Plan modals or straight
  // from the timeline/Overview panel.
  const [openTask, setOpenTask] = useState<{ key: string; title?: string } | null>(null)

  // Build channel, for dev-only surfaces (the LLM Lab). 'dev' only under
  // `tauri dev`/`cargo run` (cfg!(debug_assertions) via get_app_info) - staging
  // and prod builds never see the Lab button or modal, and its backing commands
  // are additionally refused there (commands/llm_lab.rs).
  const [channel, setChannel] = useState<string | null>(null)

  const data = useTimelineData(day)
  const { items, isSolo, connectedProviderName, connectedProviderId, isToday } = data
  const pendingCount = items.filter(isPending).length

  // Apply the persisted theme on mount (before any round-trip resolves elsewhere).
  useEffect(() => {
    load<RuntimeSettings>('/api/settings', 'get_settings')
      .then(s => applyTheme(s.theme))
      .catch(() => {})
    load<AppInfo>('/api/version', 'get_app_info')
      .then(info => setChannel(info.channel))
      .catch(() => {})
  }, [])

  // NoticeBar lives at the root layout, outside this tree, so its
  // "Fix in Tasks" CTA reaches the Tasks modal via a window event instead of
  // props.
  useEffect(() => {
    const openTasks = () => setActiveModal('tasks')
    window.addEventListener('meridian:open-tasks', openTasks)
    return () => window.removeEventListener('meridian:open-tasks', openTasks)
  }, [])

  // Tray-side openers (the daily plan auto-open, notification click-throughs)
  // steer this window to a specific view. subscribe()'s prime is the pull half
  // (`take_pending_deep_link` — a fresh window misses any event emitted before
  // its listener exists, so the tray parks the target in managed state), and
  // its listener is the push half (`dashboard-navigate`, for a window that is
  // already open and won't remount). Targets are the former route paths the
  // notification producers still use as `deep_link`s.
  useEffect(() => {
    const navigate = (target: string | null) => {
      if (target === '/plan') {
        setActiveModal('plan')
      } else if (target === '/worklogs') {
        setReviewFocusKey(null)
        setActiveModal('review')
      } else if (target === '/whats-new') {
        setActiveModal('whats-new')
      }
    }
    return subscribe<string | null>('/deep-link', 'take_pending_deep_link', 'dashboard-navigate', navigate)
  }, [])

  // Changing day resets the selected hour (its detail no longer applies).
  function shift(delta: number) {
    setSelectedHour(null)
    setSelectedCardKey(null)
    setSelectedDayTask(null)
    setDay(d => shiftDay(d, delta))
  }

  // Row-level selection (Quiet/solo rows, or blank space in a row) — shows
  // every ticket in the hour and clears any single-card selection.
  function selectHour(hour: number | null) {
    setSelectedHour(hour)
    setSelectedCardKey(null)
    // An hour's detail and a day-task's detail both own the right panel; the
    // latest click wins.
    if (hour !== null) setSelectedDayTask(null)
  }

  // A day-task claims the right panel; clear any hour detail so they don't fight
  // over it.
  function selectDayTask(detail: DayTaskDetail | null) {
    setSelectedDayTask(detail)
    if (detail) { setSelectedHour(null); setSelectedCardKey(null) }
  }

  // Closing the planner restarts the daemon's plan-nudge hold-back clock
  // (fire-and-forget; only meaningful on a day the auto-open fired — the
  // command's marker guard handles that), so the "Plan your day" reminder
  // lands an hour after the DISMISSAL, not the auto-open.
  function closePlan() {
    invoke('plan_dismissed').catch(() => {})
    setActiveModal(null)
  }

  // Marks the running app version "seen" so the once-per-version auto-open
  // (poll::whats_new_auto_open) doesn't fire again — whether this close came
  // from the auto-open or a manual nav-pill open, viewing it counts as seen.
  function closeWhatsNew() {
    invoke('mark_whats_new_seen_cmd').catch(() => {})
    setActiveModal(null)
  }

  // Opens the same swipeable Review dialog the pill/nav use, scoped to just
  // one ticket, instead of the right-side Hour-detail panel. Two callers:
  // a still-drafted card clicked directly on the timeline (TimelineColumn —
  // drafts never show in the right panel at all), and the right panel's own
  // "Edit" action on an approved/posted card (RightPanel/HourDetailPanel/
  // TimelineCard's DetailBody) — editing any state routes through this one
  // dialog rather than a separate inline editor.
  function openReview(cardKey: string) {
    setReviewFocusKey(cardKey)
    setActiveModal('review')
  }

  return (
    <div className="relative h-[100svh] overflow-hidden flex flex-col" style={{ background: 'var(--win-bg)' }}>
      <HealthBanner />

      <Toolbar
        day={day}
        isToday={isToday}
        onShiftDay={shift}
        isSolo={isSolo}
        connectedProviderName={connectedProviderName}
        connectedProviderId={connectedProviderId}
        onOpenSettings={(section) => { setSettingsSection(section); setActiveModal('settings') }}
        onOpenReport={() => setActiveModal('report')}
        showLlmLab={channel === 'dev'}
        onOpenLlmLab={() => setActiveModal('llmlab')}
        onOpenWhatsNew={() => setActiveModal('whats-new')}
      />

      <div className="flex flex-1 min-h-0">
        <div className="relative flex-1 min-w-0 min-h-0 flex flex-col">
          <DayTaskColumn day={day} isToday={isToday}
            selectedId={selectedDayTask?.id ?? null} onSelect={selectDayTask} />

          {!isSolo && (
            <FloatingDraftsPill count={pendingCount}
              onClick={() => { setReviewFocusKey(null); setActiveModal('review') }} />
          )}
        </div>
        <div className="shrink-0 border-l min-h-0" style={{ width: 388, borderColor: 'var(--t-hair)', background: 'var(--t-panel)' }}>
          <RightPanel
            data={data}
            selectedHour={selectedHour}
            selectedCardKey={selectedCardKey}
            dayTaskDetail={selectedDayTask}
            onCloseDayTask={() => setSelectedDayTask(null)}
            onSelectHour={selectHour}
            onOpen={setActiveModal}
            onOpenTask={(key, title) => setOpenTask({ key, title })}
            onEditWorklog={openReview}
            onOpenSettings={(section) => { setSettingsSection(section); setActiveModal('settings') }}
          />
        </div>
      </div>

      {activeModal === 'review' && (
        <ReviewModal items={items} actions={data.actions} focusKey={reviewFocusKey}
          onClose={() => { setActiveModal(null); setReviewFocusKey(null) }} />
      )}
      {activeModal === 'cleanup' && (
        <CleanupModal onClose={() => { setActiveModal(null); data.refetchTasks() }} />
      )}
      {activeModal === 'settings' && (
        <SettingsModal onClose={() => setActiveModal(null)} initialSection={settingsSection} />
      )}
      {activeModal === 'report' && <ReportModal onClose={() => setActiveModal(null)} />}
      {activeModal === 'llmlab' && channel === 'dev' && (
        <LlmLabModal onClose={() => setActiveModal(null)} />
      )}
      {activeModal === 'whats-new' && <WhatsNewModal onClose={closeWhatsNew} />}
      {activeModal === 'plan' && <PlanModal onClose={closePlan} />}
      {activeModal === 'tasks' && (
        <TasksModal onClose={() => setActiveModal(null)} onOpenTask={(key, title) => setOpenTask({ key, title })} />
      )}
      {openTask && (
        <TaskDetailDialog taskKey={openTask.key} fallbackTitle={openTask.title} onClose={() => setOpenTask(null)} />
      )}
    </div>
  )
}
