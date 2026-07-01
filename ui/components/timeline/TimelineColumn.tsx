//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The scrollable vertical hour timeline — one CSS Grid row per hour of the day
// (`62px 1fr`). Connected users see the hour's worklog/proposal cards stacked
// vertically; solo users (no tracker) see an activity strip of app-category
// dots + a one-line description drawn from get_today's sessions. Clicking an
// hour selects it (drives the right panel's Overview ↔ Hour-detail switch);
// the current hour carries a pulsing "now" dot.

'use client'

import { useMemo } from 'react'
import type { TodayResponse, WorklogItem } from '@/lib/api-types'
import { CATS } from '@/components/atoms'
import { hourLabel } from './timelineLayout'
import { TimelineCard } from './TimelineCard'

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
  hourBuckets, isSolo, today, selectedHour, onSelectHour, isToday,
}: {
  hourBuckets: Map<number, WorklogItem[]>
  isSolo: boolean
  today: TodayResponse | null
  selectedHour: number | null
  onSelectHour: (hour: number) => void
  isToday: boolean
}) {
  const solo = useMemo(() => soloActivityByHour(today), [today])
  const nowHour = isToday ? new Date().getHours() : -1
  const hours = Array.from({ length: 24 }, (_, h) => h)

  return (
    <div className="flex-1 min-h-0 overflow-y-auto nice-scroll">
      <div className="min-h-full">
        {hours.map(hour => {
          const items = hourBuckets.get(hour) ?? []
          const activity = solo.get(hour)
          const selected = selectedHour === hour
          const isNow = hour === nowHour
          const hasContent = isSolo ? !!activity : items.length > 0

          return (
            <div key={hour} className="grid" style={{ gridTemplateColumns: '62px 1fr' }}>
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
                className="border-t px-3 py-2.5 cursor-pointer transition-colors"
                style={{
                  borderTopColor: 'var(--t-hair)',
                  background: selected ? 'var(--t-row-hover)' : 'transparent',
                  boxShadow: selected ? 'inset 0 0 0 1px var(--row-hover-ring)' : 'none',
                }}
              >
                {!hasContent ? (
                  <p className="mt-body-sm italic py-1.5" style={{ color: 'var(--t-faint-2)' }}>Quiet</p>
                ) : isSolo && activity ? (
                  <div className="py-1 flex items-center gap-2.5">
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
                  <div className="flex flex-wrap" style={{ gap: 10 }}>
                    {items.map(w => (
                      <div key={`${w.is_proposed ? 'p' : 'w'}:${w.id}`} className="min-w-0" style={{ flex: '1 1 260px' }}>
                        <TimelineCard item={w} variant="compact" />
                      </div>
                    ))}
                  </div>
                ) : (
                  <div className="space-y-2">
                    {items.map(w => <TimelineCard key={`${w.is_proposed ? 'p' : 'w'}:${w.id}`} item={w} variant="compact" />)}
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
