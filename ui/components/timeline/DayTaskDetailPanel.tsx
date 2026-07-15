//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The right panel's task-detail state: when a day-task card is clicked in the
// timeline column, its breakdown renders HERE (in place of "Today at a glance")
// rather than as a dialog over the timeline — so the timeline keeps its clicked
// card highlighted and the rest dulled while you read the detail beside it.
// The payload is built by DayTaskColumn (which owns the fetched day-tasks) and
// threaded down through MeridianTimelineShell → RightPanel.

'use client'

import { fmtDur } from '@/components/atoms'
import { clockLabel, type LaidSegment } from './dayTaskLayout'

/** Everything the right-panel detail needs about one selected workstream — built
 *  from a `LaidOutTask` by DayTaskColumn so the panel stays free of layout math. */
export interface DayTaskDetail {
  id: string
  title: string
  minutes: number
  hue: string
  segments: LaidSegment[]
  summary: string[]
  footLo: number
  footHi: number
}

/** The selected workstream's breakdown, rendered inside the right column: its
 *  time (segment by segment, breaks called out) and its full running summary,
 *  with a Back affordance returning to the day's glance. */
export function DayTaskDetailPanel({ detail, onClose }: {
  detail: DayTaskDetail
  onClose: () => void
}) {
  const { title, minutes, hue, segments, summary, footLo, footHi } = detail
  const range = segments.length > 0 ? `${clockLabel(footLo)} - ${clockLabel(footHi)}` : ''

  return (
    <div className="dt-detail h-full overflow-y-auto nice-scroll p-6 space-y-6">
      <div>
        <button onClick={onClose}
          className="mt-body-sm inline-flex items-center gap-1.5 mb-3"
          style={{ color: 'var(--t-faint)', fontWeight: 700 }}>
          <span aria-hidden>‹</span> Back to today
        </button>
        <div className="flex items-start gap-2.5">
          <span className="mt-1.5 shrink-0 rounded-full" style={{ width: 9, height: 9, background: hue }} />
          <div className="flex-1 min-w-0">
            <p className="mt-label" style={{ color: 'var(--t-faint)' }}>Task</p>
            <p className="mt-greeting text-title mt-0.5" style={{ fontSize: 18, lineHeight: 1.3 }}>
              {title || 'Activity'}
            </p>
            <div className="flex items-center gap-2 mt-1.5">
              {minutes > 0 && (
                <span className="mt-mono-sm" style={{ fontSize: 12, fontWeight: 700, color: hue }}>
                  {fmtDur(minutes * 60)}
                </span>
              )}
              {range && (
                <span className="mt-mono-sm" style={{ fontSize: 11, color: 'var(--t-faint)' }}>
                  {range}
                </span>
              )}
            </div>
          </div>
        </div>
      </div>

      {/* When worked — one row per sitting, breaks called out between them. */}
      {segments.length > 0 && (
        <div className="rounded-xl p-4 bg-card" style={{ border: '1px solid var(--t-card-border)' }}>
          <p className="mt-label mb-2.5" style={{ color: 'var(--t-faint)' }}>When</p>
          <ul className="space-y-1">
            {segments.map((s, i) => {
              const prev = segments[i - 1]
              const gap = prev ? s.startMin - prev.endMin : 0
              return (
                <li key={i}>
                  {gap > 0 && (
                    <div className="flex items-center gap-2 my-1" style={{ paddingLeft: 2 }}>
                      <span className="mt-mono-sm" style={{ fontSize: 9.5, color: 'var(--t-faint)', opacity: 0.8 }}>
                        break · {fmtDur(gap * 60)}
                      </span>
                      <span className="flex-1 border-t border-dashed" style={{ borderColor: 'var(--t-hair)' }} />
                    </div>
                  )}
                  <div className="flex items-center gap-2.5">
                    <span className="shrink-0 rounded" style={{ width: 3, height: 14, background: hue }} />
                    <span className="mt-mono-sm" style={{ fontSize: 12, color: 'var(--t-muted)' }}>
                      {clockLabel(s.startMin)} - {clockLabel(s.endMin)}
                    </span>
                    <span className="mt-mono-sm" style={{ fontSize: 10.5, color: 'var(--t-faint)' }}>
                      {fmtDur((s.endMin - s.startMin) * 60)}
                    </span>
                  </div>
                </li>
              )
            })}
          </ul>
        </div>
      )}

      {/* What was done — the running summary log. */}
      {summary.length > 0 && (
        <div>
          <p className="mt-label mb-2" style={{ color: 'var(--t-faint)' }}>What was done</p>
          <ul className="space-y-2">
            {summary.map((line, i) => (
              <li key={i} className="mt-body-sm flex gap-2" style={{ color: 'var(--t-muted)', fontSize: 12.5, lineHeight: 1.5 }}>
                <span className="shrink-0" style={{ color: hue }}>·</span>
                <span className="flex-1">{line}</span>
              </li>
            ))}
          </ul>
        </div>
      )}
    </div>
  )
}
