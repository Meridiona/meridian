//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The right panel's hour-detail state (an hour is selected): the human-readable
// activity REPORT (get_hour_text backend data — migration 054's hour_report,
// the /activity_report LLM OUTPUT, not the raw distilled input) — solo users
// only, since it's the substitute for the PM-matched work logs a connected
// user gets instead — rendered as markdown (ActivityReport), plus the hour's
// work logs with inline Dismiss/Edit/Approve. Solo users get a dashed
// empty-state instead of work logs. A null report is EXPECTED (future/
// unprocessed hours) — it renders a placeholder, never an error. Time-by-app
// lives only in OverviewPanel — it isn't scoped per-hour/per-ticket data.

'use client'

import { fmtClock } from '@/components/atoms'
import { hourLabel } from './timelineLayout'
import { isPending, itemKey } from './types'
import { TimelineCard } from './TimelineCard'
import { ActivityReport } from './ActivityReport'
import type { TimelineData } from './useTimelineData'

export function HourDetailPanel({ hour, selectedCardKey, onBack, data, onEditWorklog }: {
  hour: number
  // When set, a specific card was clicked on the timeline — show only that
  // one ticket instead of every worklog in the hour.
  selectedCardKey: string | null
  onBack: () => void
  data: TimelineData
  onEditWorklog: (cardKey: string) => void
}) {
  const { hourBuckets, isSolo, actions, hourReports } = data

  // Still-drafted work never shows here — a draft click opens the Review
  // dialog instead (TimelineColumn/MeridianTimelineShell); this panel is for
  // already-decided (approved/posted/dismissed) work only.
  const hourItems = (hourBuckets.get(hour) ?? []).filter(w => !isPending(w))
  const items = selectedCardKey
    ? hourItems.filter(w => itemKey(w) === selectedCardKey)
    : hourItems
  // hourReports is the same top-of-app batch TimelineColumn's solo rows use
  // (useTimelineData's 30s poll) — reused here instead of a second per-hour
  // fetch, so selecting an hour shows its report instantly with no loading
  // flicker.
  const report = hourReports.find(h => h.hour === hour)?.report ?? null

  return (
    <div className="h-full overflow-y-auto nice-scroll p-6 space-y-7">
      <div>
        <button onClick={onBack} className="mt-body-sm inline-flex items-center gap-1" style={{ color: 'var(--t-muted)' }}>
          ← Overview
        </button>
        <p className="mt-greeting text-title mt-2">{hourLabel(hour)}</p>
        <p className="mt-mono-sm text-[11px] mt-0.5" style={{ color: 'var(--t-faint)' }}>
          {fmtClock(hour)} – {fmtClock(hour + 1)}
        </p>
      </div>

      {/* activity summary — the activity-report OUTPUT, not the distilled
          input. Solo-mode only: connected users get PM-matched work logs
          instead (the Section below), so this is that surface's substitute,
          not an addition to it. */}
      {isSolo && (
        <Section label="Activity summary">
          {report ? (
            <div className="rounded-xl p-5 bg-box">
              <ActivityReport report={report} />
              <p className="mt-label mt-3" style={{ color: 'var(--t-faint)' }}>◈ Captured from screen · accessibility tree + OCR</p>
            </div>
          ) : (
            <p className="mt-body-sm italic" style={{ color: 'var(--t-faint-2)' }}>Not yet available for this hour.</p>
          )}
        </Section>
      )}

      {/* work logs, or the solo empty-state */}
      <Section label={isSolo ? 'Work logs' : `Work logs${items.length ? ` · ${items.length}` : ''}`}>
        {isSolo ? (
          <div className="rounded-xl p-5 text-center" style={{ border: '1px dashed var(--t-hair)' }}>
            <p className="mt-title text-title">No work logs in Solo mode</p>
            <p className="mt-body-sm mt-1.5" style={{ color: 'var(--t-muted)' }}>
              Connect a tracker to turn this hour&apos;s activity into matched work logs automatically.
            </p>
          </div>
        ) : items.length === 0 ? (
          <p className="mt-body-sm italic" style={{ color: 'var(--t-faint-2)' }}>Nothing logged this hour.</p>
        ) : (
          <div className="space-y-3">
            {items.map(w => (
              <TimelineCard key={itemKey(w)} item={w} variant="detail" actions={actions}
                onEdit={() => onEditWorklog(itemKey(w))} />
            ))}
          </div>
        )}
      </Section>
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
