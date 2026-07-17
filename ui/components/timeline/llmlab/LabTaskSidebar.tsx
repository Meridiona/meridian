//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The LLM Lab's task sidebar: when a task card is clicked in a fold variant's
// timeline (VariantBody -> DayTaskColumn), its breakdown renders HERE, beside the
// timeline. Read-only by design and DELIBERATELY not DayTaskDetailPanel - that
// panel drives useWorklog (get / generate / approve) against the PRODUCTION
// day_task_worklogs tables for a task id, and a fold's task ids are a model's
// SIMULATED day, not real rows. So this shows only what the fold result itself
// carries: when the task was worked, and the model's own per-task log. The
// "draft with this model" action (a metered, on-demand call) is wired in Phase B.
// Dev-only surface - plain hyphens in all copy.

'use client'

import { fmtDur } from '@/components/atoms'
import { clockLabel } from '../dayTaskLayout'
import { Bullets, Field } from '../dayTaskKit'
import type { DayTaskDetail } from '../DayTaskDetailPanel'

export function LabTaskSidebar({ detail, onClose }: {
  detail: DayTaskDetail
  onClose: () => void
}) {
  const { title, hue, minutes, segments, summary, footLo, footHi } = detail
  const range = segments.length > 0 ? `${clockLabel(footLo)} - ${clockLabel(footHi)}` : ''
  const sittings = segments.length

  return (
    <div className="shrink-0 border-l flex flex-col min-h-0"
      style={{ width: 340, borderColor: 'var(--t-hair)', background: 'var(--t-box)' }}>
      {/* header: task title + close */}
      <div className="flex items-start gap-2 px-4 py-3 border-b shrink-0" style={{ borderColor: 'var(--t-hair)' }}>
        <span className="shrink-0 rounded-full mt-1" style={{ width: 8, height: 8, background: hue }} />
        <div className="flex-1 min-w-0">
          <p className="mt-label" style={{ color: 'var(--t-faint)' }}>TASK</p>
          <p className="mt-card-title" style={{ color: 'var(--t-title)', lineHeight: 1.3 }}>
            {title || 'Activity'}
          </p>
        </div>
        <button onClick={onClose} aria-label="Close"
          className="inline-flex items-center justify-center rounded-full bg-wrap shrink-0"
          style={{ width: 26, height: 26, color: 'var(--t-muted)' }}>
          <span className="text-[15px] leading-none">×</span>
        </button>
      </div>

      {/* scrolling detail */}
      <div className="flex-1 min-h-0 overflow-y-auto nice-scroll px-4 py-4 space-y-4">
        <Field label="WHEN">
          <p className="mt-body-sm" style={{ color: 'var(--t-muted)' }}>
            {range || 'No timing recorded'}
            {minutes > 0 && <span style={{ color: 'var(--t-faint)' }}> · {fmtDur(minutes * 60)}</span>}
            {sittings > 1 && <span style={{ color: 'var(--t-faint)' }}> · {sittings} sittings</span>}
          </p>
        </Field>

        <Field label="WHAT WAS DONE">
          {summary.length > 0 ? (
            <Bullets items={summary} accent={hue} size={12.5} />
          ) : (
            <p className="mt-body-sm" style={{ color: 'var(--t-faint)' }}>
              This model logged no per-task detail.
            </p>
          )}
          <p className="mt-body-sm mt-2" style={{ color: 'var(--t-faint)', fontSize: 10.5 }}>
            This is the model's own per-task log - Lab output, not posted.
          </p>
        </Field>

        {/* Phase B: the on-demand "Draft worklog with this model" action lands here. */}
      </div>
    </div>
  )
}
