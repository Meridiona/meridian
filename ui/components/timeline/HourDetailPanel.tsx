//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The right panel's hour-detail state (an hour is selected): the distilled
// activity summary (new get_hour_text backend data — migration 053), a
// time-by-app chart scoped to the hour, and the hour's work logs with inline
// Dismiss/Edit/Approve. Solo users get a dashed empty-state instead of work
// logs. A null hour-text body is EXPECTED (future/unprocessed hours) — it
// renders a placeholder, never an error.

'use client'

import { useEffect, useState } from 'react'
import { fmtClock } from '@/components/atoms'
import { load } from '@/lib/bridge'
import type { HourTextResponse, TodaySession } from '@/lib/api-types'
import { hourLabel } from './timelineLayout'
import { TimeByApp } from './TimeByApp'
import { TimelineCard } from './TimelineCard'
import type { TimelineData } from './useTimelineData'

export function HourDetailPanel({ hour, onBack, data }: {
  hour: number
  onBack: () => void
  data: TimelineData
}) {
  const { day, hourBuckets, today, isSolo, actions } = data
  const [hourText, setHourText] = useState<HourTextResponse | null>(null)
  const [loadingText, setLoadingText] = useState(true)

  useEffect(() => {
    setLoadingText(true)
    setHourText(null)
    load<HourTextResponse>('/api/hour-text', 'get_hour_text', { day, hour: String(hour) })
      .then(setHourText)
      .catch(() => setHourText(null))
      .finally(() => setLoadingText(false))
  }, [day, hour])

  const items = hourBuckets.get(hour) ?? []
  const hourSessions: TodaySession[] = (today?.sessions ?? []).filter(
    s => new Date(s.started_at).getHours() === hour,
  )
  const body = hourText?.body ?? null

  return (
    <div className="h-full overflow-y-auto nice-scroll p-5 space-y-6">
      <div>
        <button onClick={onBack} className="mt-body-sm inline-flex items-center gap-1" style={{ color: 'var(--t-muted)' }}>
          ← Overview
        </button>
        <p className="mt-greeting text-title mt-2">{hourLabel(hour)}</p>
        <p className="mt-mono-sm text-[11px] mt-0.5" style={{ color: 'var(--t-faint)' }}>
          {fmtClock(hour)} – {fmtClock(hour + 1)}
        </p>
      </div>

      {/* activity summary — the new hour-text data */}
      <Section label="Activity summary">
        {loadingText ? (
          <p className="mt-body-sm italic" style={{ color: 'var(--t-faint-2)' }}>Loading…</p>
        ) : body ? (
          <div className="rounded-xl p-4 bg-box">
            <p className="mt-body whitespace-pre-wrap" style={{ color: 'var(--t-title)' }}>{body}</p>
            <p className="mt-label mt-3" style={{ color: 'var(--t-faint)' }}>◈ Captured from screen · accessibility tree + OCR</p>
          </div>
        ) : (
          <p className="mt-body-sm italic" style={{ color: 'var(--t-faint-2)' }}>Not yet available for this hour.</p>
        )}
      </Section>

      <Section label="Time by app">
        <TimeByApp sessions={hourSessions} />
      </Section>

      {/* work logs, or the solo empty-state */}
      <Section label={isSolo ? 'Work logs' : `Work logs${items.length ? ` · ${items.length}` : ''}`}>
        {isSolo ? (
          <div className="rounded-xl p-4 text-center" style={{ border: '1px dashed var(--t-hair)' }}>
            <p className="mt-title text-title">No work logs in Solo mode</p>
            <p className="mt-body-sm mt-1.5" style={{ color: 'var(--t-muted)' }}>
              Connect a tracker to turn this hour&apos;s activity into matched work logs automatically.
            </p>
          </div>
        ) : items.length === 0 ? (
          <p className="mt-body-sm italic" style={{ color: 'var(--t-faint-2)' }}>Nothing logged this hour.</p>
        ) : (
          <div className="space-y-2.5">
            {items.map(w => (
              <TimelineCard key={`${w.is_proposed ? 'p' : 'w'}:${w.id}`} item={w} variant="detail" actions={actions} />
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
