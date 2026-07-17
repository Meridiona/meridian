//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// One LLM Lab variant's rendered outcome, drawn full-width for the single-timeline
// view (RunView). For a fold process (workstream_fold / day_fold) a successful
// variant's output_rendered is a DayTasksResponse - it is fed straight into the
// REAL DayTaskColumn, so the pane shows the day's actual dashboard timeline as
// that model would have built it, and clicking a card drives the run's shared
// task sidebar (selection is owned by RunView, not here). Other processes render
// text with a raw toggle; failed / rate-limited / pending variants show their
// status instead of an empty pane. Extracted from the former ResultsGrid's
// VariantColumn. Dev-only surface - plain hyphens in all copy.

'use client'

import { useMemo, useState } from 'react'
import type { DayTasksResponse, LlmExperimentResult } from '@/lib/api-types'
import { DayTaskColumn } from '../DayTaskColumn'
import type { DayTaskDetail } from '../DayTaskDetailPanel'

export const STATUS_META: Record<string, { label: string; color: string }> = {
  pending: { label: 'Queued', color: 'var(--t-faint)' },
  running: { label: 'Running…', color: 'var(--color-state-pending)' },
  ok: { label: 'OK', color: 'var(--color-state-approved)' },
  failed: { label: 'Failed', color: '#E11D48' },
  rate_limited: { label: 'Rate limited', color: '#E11D48' },
}

/** The two processes whose rendered output is a day-task set the real timeline
 *  can draw. */
export function isFoldProcess(process: string): boolean {
  return process === 'workstream_fold' || process === 'day_fold'
}

/** "12.3s" / "3m 05s" from elapsed seconds. */
function fmtElapsed(s: number | null): string {
  if (s === null || s <= 0) return '-'
  if (s < 60) return `${s.toFixed(1)}s`
  const m = Math.floor(s / 60)
  const rest = Math.round(s % 60)
  return `${m}m ${String(rest).padStart(2, '0')}s`
}

/** Token count - the CLI backends report 0 (they expose none), shown as "-". */
function fmtTokens(n: number | null): string {
  return n && n > 0 ? String(n) : '-'
}

/** A fold variant's rendered payload: the day-task set (+ optional honesty note,
 *  e.g. "no usable placements" / "stopped at <hour>"). */
type FoldRender = (DayTasksResponse & { note?: string }) | null

/** One variant's body, full-width. `selectedTask` / `onSelectTask` are the run's
 *  shared task selection (owned by RunView) - clicking a fold card opens the
 *  sidebar beside this pane. */
export function VariantBody({ result, fold, selectedTask, onSelectTask }: {
  result: LlmExperimentResult
  fold: boolean
  selectedTask: DayTaskDetail | null
  onSelectTask: (detail: DayTaskDetail | null) => void
}) {
  const [showRaw, setShowRaw] = useState(false)

  // Parse the fold payload once per result; a parse failure falls back to text.
  const foldRender: FoldRender = useMemo(() => {
    if (!fold || result.status !== 'ok' || !result.output_rendered) return null
    try {
      const v = JSON.parse(result.output_rendered) as FoldRender
      return v && Array.isArray(v.tasks) ? v : null
    } catch {
      return null
    }
  }, [fold, result.status, result.output_rendered])

  const timeline = foldRender !== null && !showRaw

  return (
    <div className="flex-1 min-h-0 flex flex-col">
      {/* metrics row */}
      <div className="flex items-center gap-3 px-4 py-2 border-b shrink-0"
        style={{ borderColor: 'var(--t-hair)', color: 'var(--t-faint)', fontSize: 11 }}>
        <span>time {fmtElapsed(result.elapsed_s)}</span>
        <span>tokens in {fmtTokens(result.input_tokens)} / out {fmtTokens(result.output_tokens)}</span>
        {result.status === 'ok' && (
          <button onClick={() => setShowRaw(v => !v)} className="ml-auto rounded px-1.5 py-0.5"
            style={{
              border: '1px solid var(--t-ctrl-border)', fontSize: 10, cursor: 'pointer',
              background: showRaw ? 'var(--t-wrap)' : 'var(--t-ctrl)', color: 'var(--t-muted)',
            }}>
            {showRaw ? (foldRender ? 'timeline' : 'rendered') : 'raw'}
          </button>
        )}
      </div>

      {/* honesty note from the renderer (kept prior state / partial day) */}
      {timeline && foldRender?.note && (
        <p className="px-4 py-1.5 border-b shrink-0"
          style={{ borderColor: 'var(--t-hair)', color: 'var(--color-state-pending)', fontSize: 11 }}>
          {foldRender.note}
        </p>
      )}

      {/* body */}
      {timeline && foldRender ? (
        // The REAL dashboard timeline, fed this variant's simulated day. Selection
        // is the run's shared state, so a click here opens the task sidebar.
        <div className="flex-1 min-h-0">
          <DayTaskColumn
            day={foldRender.day}
            isToday={false}
            selectedId={selectedTask?.id ?? null}
            onSelect={onSelectTask}
            tasks={foldRender.tasks}
          />
        </div>
      ) : (
        <div className="flex-1 min-h-0 overflow-y-auto nice-scroll px-5 py-4">
          {result.status === 'ok' && (
            <pre className="whitespace-pre-wrap break-words"
              style={{ font: '400 12px/1.6 var(--font-mono, ui-monospace)', color: 'var(--t-muted)', margin: 0, maxWidth: 760 }}>
              {(showRaw ? result.output_text : result.output_rendered) ?? '(no output recorded)'}
            </pre>
          )}
          {(result.status === 'failed' || result.status === 'rate_limited') && (
            <>
              <p className="mt-body-sm whitespace-pre-wrap break-words" style={{ color: '#E11D48' }}>
                {result.error ?? 'No error message recorded.'}
              </p>
              {/* A failed day-fold still carries the partial day it built. */}
              {result.output_rendered && fold && (
                <pre className="whitespace-pre-wrap break-words mt-2"
                  style={{ font: '400 11px/1.5 var(--font-mono, ui-monospace)', color: 'var(--t-faint)', margin: 0 }}>
                  {result.output_rendered}
                </pre>
              )}
            </>
          )}
          {(result.status === 'pending' || result.status === 'running') && (
            <p className="mt-body-sm" style={{ color: 'var(--t-faint)' }}>
              {result.status === 'pending'
                ? 'Waiting for its turn - variants run one at a time.'
                : fold
                  ? 'Folding hour by hour - a full day takes a few minutes per model.'
                  : 'The model is thinking - this can take a few minutes.'}
            </p>
          )}
        </div>
      )}
    </div>
  )
}
