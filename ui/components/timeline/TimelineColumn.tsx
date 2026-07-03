//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The scrollable vertical hour timeline — one CSS Grid row per hour of the day
// (`62px 1fr`). Connected users see the hour's worklog/proposal cards stacked
// vertically; solo users (no tracker) get the hour's real activity-report
// markdown rendered filling the whole block (get_hour_reports — the same
// /activity_report LLM output the hour-detail panel shows), falling back to
// an app-category-dots + one-line summary (drawn from get_today's sessions)
// only while that hour's report hasn't been generated yet. Clicking an hour
// selects it (drives the right panel's Overview ↔ Hour-detail switch); the
// current hour carries a pulsing "now" dot.

'use client'

import { useEffect, useMemo, useRef } from 'react'
import type { HourReportEntry, HourStatus, TodayResponse, WorklogItem } from '@/lib/api-types'
import { CATS } from '@/components/atoms'
import { hourLabel } from './timelineLayout'
import { isPending, itemKey } from './types'
import { TimelineCard } from './TimelineCard'
import { HourBadges, HourTakeover } from './HourBadges'
import { ActivityReport } from './ActivityReport'

interface HourActivity {
  cats: string[]        // distinct category keys seen this hour (for the dots)
  label: string         // one-line description, or '' when quiet
}

/** Bucket get_today sessions into a per-hour activity summary for solo mode. */
function soloActivityByHour(today: TodayResponse | null): Map<number, HourActivity> {
  const out = new Map<number, HourActivity>()
  if (!today) return out
  const byHour = new Map<number, TodayResponse['sessions']>()
  for (const s of today.sessions) {
    const h = new Date(s.started_at).getHours()
    if (Number.isNaN(h)) continue
    if (!byHour.has(h)) byHour.set(h, [])
    byHour.get(h)!.push(s)
  }
  for (const [h, sessions] of byHour) {
    const cats: string[] = []
    for (const s of sessions) if (!cats.includes(s.cat)) cats.push(s.cat)
    // Prefer a session summary lead; else the top app names this hour.
    const summarised = sessions.find(s => s.summary?.trim())
    const apps: string[] = []
    for (const s of sessions) if (s.app && !apps.includes(s.app)) apps.push(s.app)
    const label = summarised?.summary?.trim().split(/[.!?]/)[0]?.slice(0, 90)
      || apps.slice(0, 3).join(' · ')
    out.set(h, { cats: cats.slice(0, 6), label: label || '' })
  }
  return out
}

export function TimelineColumn({
  hourBuckets, isSolo, today, selectedHour, selectedCardKey, onSelectHour, onSelectCard,
  onOpenDraftReview, isToday, day, hourStatus, capturing, hourReports,
}: {
  hourBuckets: Map<number, WorklogItem[]>
  isSolo: boolean
  today: TodayResponse | null
  selectedHour: number | null
  selectedCardKey: string | null
  // Approved/posted/dismissed cards only — narrows the right-side Hour-detail
  // panel to that one card.
  onSelectCard: (hour: number, cardKey: string) => void
  // Still-drafted (pending) cards only — opens the swipeable Review dialog
  // scoped to that one card instead of the right panel (drafts are reviewed/
  // approved there, not read in the side panel — see ReviewOverlay's
  // `focusKey`).
  onOpenDraftReview: (cardKey: string) => void
  onSelectHour: (hour: number) => void
  isToday: boolean
  day: string
  hourStatus: HourStatus[]
  capturing: boolean | null
  hourReports: HourReportEntry[]
}) {
  const solo = useMemo(() => soloActivityByHour(today), [today])
  const hourStatusByHour = useMemo(() => new Map(hourStatus.map(h => [h.hour, h])), [hourStatus])
  const hourReportByHour = useMemo(() => new Map(hourReports.map(h => [h.hour, h.report])), [hourReports])
  const nowHour = isToday ? new Date().getHours() : -1
  const hours = Array.from({ length: 24 }, (_, h) => h)

  // On opening today's view (or switching back to today), jump straight to the
  // current local hour instead of leaving the scroll at midnight/top.
  const rowRefs = useRef<Map<number, HTMLDivElement>>(new Map())
  useEffect(() => {
    if (!isToday || nowHour < 0) return
    const el = rowRefs.current.get(nowHour)
    el?.scrollIntoView({ block: 'center' })
  }, [day, isToday, nowHour])

  return (
    <div className="flex-1 min-w-0 min-h-0 overflow-y-auto overflow-x-hidden nice-scroll">
      <div className="min-h-full px-6">
        {hours.map(hour => {
          const items = hourBuckets.get(hour) ?? []
          const activity = solo.get(hour)
          // The actual /activity_report markdown for this hour — the primary
          // solo-mode content once it exists; `activity` (the coarse
          // session-derived one-liner) is only a fallback while the report
          // hasn't been generated yet.
          const report = isSolo ? hourReportByHour.get(hour) ?? null : null
          // Row-level highlight only applies when the hour itself (not one of
          // its cards) is the current selection — a card click "pops" the card
          // forward instead (see TimelineCard's `selected` prop below).
          const rowSelected = selectedHour === hour && !selectedCardKey
          const isNow = hour === nowHour
          const hasContent = isSolo ? !!report || !!activity : items.length > 0
          const status = hourStatusByHour.get(hour)
          const generating = !!status?.generating
          // Live pause only paints the CURRENT hour (that's the hour tracking
          // is actually paused during, right now); every other paused hour
          // shown is historical, from the gaps table.
          const pausedNow = isNow && capturing === false
          const pausedHistoric = !!status?.paused && !pausedNow
          // The current hour hasn't ended yet, so it can never be `generating`
          // — it's simply next in line, drafted once the clock crosses into
          // the following hour. See HourBadges' doc comment for why this
          // matters (the DB `generating` status is otherwise near-invisible).
          const queued = isNow && !generating
          const takeoverMode = generating ? 'generating' as const : queued ? 'queued' as const : null
          const nextHourLabel = hourLabel((hour + 1) % 24)

          return (
            <div key={hour} ref={el => { if (el) rowRefs.current.set(hour, el); else rowRefs.current.delete(hour) }}
              className="grid" style={{ gridTemplateColumns: '62px 1fr' }}>
              {/* hour gutter */}
              <div className="relative flex items-start justify-end pr-3 pt-3">
                <span className="mt-mono-sm text-[11px]" style={{ color: isNow ? 'var(--color-state-pending)' : 'var(--t-faint)' }}>
                  {hourLabel(hour)}
                </span>
                {isNow && (
                  <span className="absolute right-0 top-3.5 -mr-1 inline-block w-2 h-2 rounded-full live-dot"
                    style={{ background: 'var(--color-state-pending)' }} aria-label="current hour" />
                )}
              </div>

              {/* clickable content area */}
              <div
                role="button"
                tabIndex={0}
                onClick={() => onSelectHour(hour)}
                onKeyDown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); onSelectHour(hour) } }}
                className="min-w-0 border-t px-4 py-5 cursor-pointer transition-colors"
                style={{
                  borderTopColor: 'var(--t-hair)',
                  background: rowSelected ? 'var(--t-row-hover)' : 'transparent',
                  boxShadow: rowSelected ? 'inset 0 0 0 1px var(--row-hover-ring)' : 'none',
                }}
              >
                {takeoverMode ? (
                  // The whole row BLOCKS out — an unmistakable takeover, not a
                  // small badge tucked in a corner — for both the live current
                  // hour (`queued`, always visible) and the brief real
                  // `generating` HTTP-call window.
                  <HourTakeover hour={hour} mode={takeoverMode} paused={pausedNow || pausedHistoric} nextHourLabel={nextHourLabel} isSolo={isSolo} />
                ) : (
                  <div className="flex items-start gap-3">
                    <div className="min-w-0 flex-1">
                      {!hasContent ? (
                        <p className="mt-body-sm italic py-1.5" style={{ color: 'var(--t-faint-2)' }}>Quiet</p>
                      ) : isSolo && report ? (
                        // The whole block renders the hour's actual activity
                        // report — no truncation — once it's been generated.
                        <div className="py-1">
                          <ActivityReport report={report} compact />
                        </div>
                      ) : isSolo && activity ? (
                        // Report not generated yet — fall back to the coarse
                        // session-derived one-liner so the row isn't empty.
                        <div className="py-1 flex items-center gap-3">
                          <span className="flex items-center gap-1 shrink-0">
                            {activity.cats.map(c => (
                              <span key={c} className={`inline-block w-2 h-2 rounded-full cat-${c}`} title={CATS[c]?.label ?? c} />
                            ))}
                          </span>
                          <span className="mt-body-sm truncate" style={{ color: 'var(--t-muted)' }}>
                            {activity.label || 'Activity captured'}
                          </span>
                        </div>
                      ) : items.length >= 2 ? (
                        // STYLESHEET.md §7 "Two tickets in an hour": flex row, gap 10px, equal 1fr columns.
                        // Generalized to any count — wraps to a new row when there isn't room for
                        // another column, rather than only side-by-side at exactly 2.
                        <div className="flex flex-wrap" style={{ gap: 16 }}>
                          {items.map(w => {
                            const key = itemKey(w)
                            return (
                              <div key={key} className="min-w-0" style={{ flex: '1 1 260px' }}
                                onClick={(e) => {
                                  e.stopPropagation()
                                  isPending(w) ? onOpenDraftReview(key) : onSelectCard(hour, key)
                                }}>
                                <TimelineCard item={w} variant="compact" selected={key === selectedCardKey} />
                              </div>
                            )
                          })}
                        </div>
                      ) : (
                        <div className="space-y-4">
                          {items.map(w => {
                            const key = itemKey(w)
                            return (
                              <div key={key} onClick={(e) => {
                                e.stopPropagation()
                                isPending(w) ? onOpenDraftReview(key) : onSelectCard(hour, key)
                              }}>
                                <TimelineCard item={w} variant="compact" selected={key === selectedCardKey} />
                              </div>
                            )
                          })}
                        </div>
                      )}
                    </div>
                    <HourBadges pausedNow={pausedNow} pausedHistoric={pausedHistoric} />
                  </div>
                )}
              </div>
            </div>
          )
        })}
      </div>
    </div>
  )
}
