//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The right panel's default state (no hour selected): a greeting, the day's
// time-by-app chart, three insight mini-cards, and — for connected users —
// drafts-to-review and board-cleanup CTAs. A Tasks entry point sits at the
// bottom (per product decision, Tasks is a modal, not a route). Narrative +
// metric fields are adapted from the retired TodayView's data; no new backend
// calls.

'use client'

import { fmtDur } from '@/components/atoms'
import { isPending } from './types'
import { TimeByApp, appTotals } from './TimeByApp'
import type { TimelineData } from './useTimelineData'
import type { ActiveModal } from './MeridianTimelineShell'

export function OverviewPanel({ data, onOpen }: {
  data: TimelineData
  onOpen: (modal: ActiveModal) => void
}) {
  const { today, isSolo, items, counts, cleanupIssueCount, tasks } = data
  const pendingCount = items.filter(isPending).length
  const focus_s = today?.focus_s ?? 0
  const meeting_s = today ? today.sessions.filter(s => s.cat === 'meeting').reduce((a, s) => a + s.dur, 0) : 0
  const switches = today?.switch_count ?? 0
  const appCount = today ? appTotals(today.sessions).length : 0
  const loggedCount = (counts.approved ?? 0) + (counts.posted ?? 0)
  const activeTaskCount = tasks.filter(t => !t.is_terminal).length

  const greetingTitle = isSolo ? 'Your day, captured' : 'Your day so far'
  const greetingBody = isSolo
    ? `${fmtDur(focus_s)} of focused activity across ${appCount} app${appCount === 1 ? '' : 's'}.`
    : `${loggedCount} work log${loggedCount === 1 ? '' : 's'} logged · ${fmtDur(focus_s)} focused.`

  return (
    <div className="h-full overflow-y-auto nice-scroll p-5 space-y-6">
      <div>
        <p className="mt-greeting text-title">{greetingTitle}</p>
        <p className="mt-body mt-1" style={{ color: 'var(--t-muted)' }}>{greetingBody}</p>
      </div>

      <Section label="Time by app">
        <TimeByApp sessions={today?.sessions ?? []} />
      </Section>

      <div className="grid grid-cols-3 gap-2.5">
        <Mini label="Focus" value={fmtDur(focus_s)} tint="var(--color-state-proposal)" />
        <Mini label="Meetings" value={fmtDur(meeting_s)} tint="var(--color-state-pending)" />
        <Mini label="Switches" value={String(switches)} tint="#0EA5A0" />
      </div>

      {!isSolo && pendingCount > 0 && (
        <button onClick={() => onOpen('review')}
          className="w-full text-left rounded-xl p-4 transition-transform active:scale-[.99]"
          style={{ background: 'var(--chip)', color: '#fff' }}>
          <p className="mt-title-lg">{pendingCount} draft{pendingCount === 1 ? '' : 's'} to review</p>
          <p className="mt-body-sm mt-1" style={{ opacity: 0.9 }}>Swipe through and approve or dismiss →</p>
        </button>
      )}

      {!isSolo && cleanupIssueCount > 0 && (
        <button onClick={() => onOpen('cleanup')}
          className="w-full text-left rounded-xl p-4 bg-card"
          style={{ border: '1px solid var(--t-card-border)' }}>
          <p className="mt-title" style={{ color: 'var(--color-state-pending)' }}>🧹 {cleanupIssueCount} board issue{cleanupIssueCount === 1 ? '' : 's'}</p>
          <p className="mt-body-sm mt-1" style={{ color: 'var(--t-muted)' }}>Tidy up tickets so time attributes cleanly →</p>
        </button>
      )}

      {/* entry points — Tasks + Daily plan (both open as modals) */}
      <div className="space-y-2.5">
        {!isSolo && (
          <EntryRow label="Daily plan" hint="Plan today" onClick={() => onOpen('plan')} />
        )}
        <EntryRow label="Tasks" hint={`${activeTaskCount} active`} onClick={() => onOpen('tasks')} />
      </div>
    </div>
  )
}

function Section({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div>
      <p className="mt-label mb-2.5" style={{ color: 'var(--t-faint)' }}>{label}</p>
      {children}
    </div>
  )
}

function EntryRow({ label, hint, onClick }: { label: string; hint: string; onClick: () => void }) {
  return (
    <button onClick={onClick}
      className="w-full flex items-center gap-3 rounded-xl px-4 py-3 bg-card"
      style={{ border: '1px solid var(--t-card-border)' }}>
      <span className="mt-title text-title flex-1 text-left">{label}</span>
      <span className="mt-mono-sm text-[11px]" style={{ color: 'var(--t-faint)' }}>{hint}</span>
      <span style={{ color: 'var(--t-faint)' }}>›</span>
    </button>
  )
}

function Mini({ label, value, tint }: { label: string; value: string; tint: string }) {
  return (
    <div className="rounded-xl p-3 bg-box">
      <p className="mt-stat" style={{ color: tint }}>{value}</p>
      <p className="mt-label mt-1" style={{ color: 'var(--t-faint)' }}>{label}</p>
    </div>
  )
}
