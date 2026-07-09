//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The right panel's hour-detail state (an hour is selected): the human-readable
// activity REPORT (get_hour_text backend data — migration 054's hour_report,
// the /activity_report LLM OUTPUT, not the raw distilled input) — solo users
// only, since it's the substitute for the PM-matched work logs a connected
// user gets instead. Connected users get the hour's work logs (with inline
// Dismiss/Edit/Approve) instead — solo mode has no tracker to match against,
// so that section doesn't render at all rather than showing an empty state
// alongside the Activity summary. A null report is EXPECTED (future/
// unprocessed hours) — it renders a placeholder, never an error. Time-by-app
// lives only in OverviewPanel — it isn't scoped per-hour/per-ticket data.

'use client'

import { useState } from 'react'
import { fmtClock } from '@/components/atoms'
import { hourLabel } from './timelineLayout'
import { isPending, itemKey } from './types'
import { TimelineCard } from './TimelineCard'
import { ActivityReport, extractCoreTaskNames, extractTldr } from './ActivityReport'
import { ActivityReportModal } from './ActivityReportModal'
import { CurvedArrow } from './CurvedArrow'
import type { TimelineData } from './useTimelineData'
import type { SettingsSection } from './settings/types'

export function HourDetailPanel({ hour, selectedCardKey, onBack, data, onEditWorklog, onOpenSettings }: {
  hour: number
  // When set, a specific card was clicked on the timeline — show only that
  // one ticket instead of every worklog in the hour.
  selectedCardKey: string | null
  onBack: () => void
  data: TimelineData
  onEditWorklog: (cardKey: string) => void
  // Deep-link into Settings → Integrations — the solo-mode "Connect a
  // tracker" CTA below uses this.
  onOpenSettings: (section?: SettingsSection) => void
}) {
  const { hourBuckets, isSolo, actions, hourReports } = data
  const [showFullReport, setShowFullReport] = useState(false)

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
        <Section label="Activity summary" emphasize>
          {report ? (
            <div className="rounded-xl p-5 bg-box space-y-4">
              <ActivityReport report={extractTldr(report)} />
              {(() => {
                const taskNames = extractCoreTaskNames(report)
                return taskNames.length > 0 ? (
                  <div>
                    <p className="mt-label mb-2" style={{ color: 'var(--t-faint)' }}>Core Tasks &amp; Projects</p>
                    <ul className="space-y-1.5" style={{ listStyle: 'none', paddingLeft: 0 }}>
                      {taskNames.map((name, i) => (
                        <li key={i} className="mt-body-sm flex items-center gap-2" style={{ color: 'var(--t-title)' }}>
                          <span aria-hidden="true" style={{ color: 'var(--t-faint)' }}>·</span>
                          <span className="min-w-0">{name}</span>
                        </li>
                      ))}
                    </ul>
                  </div>
                ) : null
              })()}
              <button onClick={() => setShowFullReport(true)}
                className="mt-body-sm inline-flex items-center gap-1"
                style={{ color: 'var(--color-state-proposal)', fontWeight: 700 }}>
                View full report ↗
              </button>
            </div>
          ) : (
            <p className="mt-body-sm italic" style={{ color: 'var(--t-faint-2)' }}>Not yet available for this hour.</p>
          )}

          {report && (
            <>
              <div className="flex justify-start pl-4 -mb-2" aria-hidden="true">
                <CurvedArrow size={36} />
              </div>
              <button onClick={() => onOpenSettings('integrations')}
                className="w-full flex items-center gap-3 rounded-xl px-4 py-3 text-left transition-colors mt-card-hover"
                style={{
                  border: '1px dashed color-mix(in srgb, var(--color-state-proposal) 45%, transparent)',
                  background: 'color-mix(in srgb, var(--color-state-proposal) 6%, transparent)',
                }}>
                <span className="flex-1 min-w-0">
                  <p className="mt-body-sm" style={{ color: 'var(--t-title)', fontWeight: 700 }}>Want this logged to your PM app?</p>
                  <p className="mt-body-sm mt-0.5" style={{ color: 'var(--t-muted)' }}>Connect a tracker and Meridian matches your work automatically.</p>
                </span>
              </button>
            </>
          )}
        </Section>
      )}

      {showFullReport && report && (
        <ActivityReportModal hour={hour} report={report} onClose={() => setShowFullReport(false)} />
      )}

      {/* Work logs — connected users only. Solo mode has no tracker to match
          against, so there's nothing to show here; Activity summary above is
          the whole story for that hour, not a section alongside an empty one. */}
      {!isSolo && (
        <Section label={`Work logs${items.length ? ` · ${items.length}` : ''}`}>
          {items.length === 0 ? (
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
      )}
    </div>
  )
}

// `emphasize` (Activity summary only — the report is the primary content in
// solo mode) swaps the standard faint uppercase eyebrow for a bold title-color
// heading; `Work logs` keeps the plain eyebrow style.
function Section({ label, children, emphasize = false }: { label: string; children: React.ReactNode; emphasize?: boolean }) {
  return (
    <div>
      {emphasize ? (
        <p className="mb-2.5" style={{ font: "800 13.5px 'Plus Jakarta Sans', var(--font-pjs), sans-serif", color: 'var(--t-title)' }}>
          {label}
        </p>
      ) : (
        <p className="mt-label mb-2.5" style={{ color: 'var(--t-faint)' }}>{label}</p>
      )}
      {children}
    </div>
  )
}
