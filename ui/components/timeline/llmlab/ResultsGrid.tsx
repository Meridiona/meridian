//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The LLM Lab's side-by-side comparison: one column per provider/model variant,
// each showing its outcome badge, timing/token metrics, and the output. For the
// fold processes (workstream_fold / day_fold) a successful variant's
// output_rendered is a DayTasksResponse - it is fed straight into the REAL
// DayTaskColumn, so each column shows the day's actual dashboard timeline as
// that model would have built it. Other processes render text; every column has
// a raw toggle. Dev-only surface - plain hyphens in all copy.

'use client'

import { useMemo, useState } from 'react'
import type { DayTasksResponse, LlmExperimentDetail, LlmExperimentResult } from '@/lib/api-types'
import { llmProvider, type LlmProviderId } from '@/lib/llm-providers'
import { DayTaskColumn } from '../DayTaskColumn'

const STATUS_META: Record<string, { label: string; color: string }> = {
  pending: { label: 'Queued', color: 'var(--t-faint)' },
  running: { label: 'Running…', color: 'var(--color-state-pending)' },
  ok: { label: 'OK', color: 'var(--color-state-approved)' },
  failed: { label: 'Failed', color: '#E11D48' },
  rate_limited: { label: 'Rate limited', color: '#E11D48' },
}

/** The two processes whose rendered output is a day-task set the real timeline
 *  can draw. */
function isFoldProcess(process: string): boolean {
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

export function ResultsGrid({ detail }: { detail: LlmExperimentDetail }) {
  const fold = isFoldProcess(detail.process)
  return (
    <div className="flex-1 min-h-0 flex gap-3 overflow-x-auto nice-scroll" style={{ padding: 2 }}>
      {detail.results.map(r => (
        <VariantColumn key={r.variant_idx} result={r} fold={fold} />
      ))}
    </div>
  )
}

/** A fold variant's rendered payload: the day-task set (+ optional honesty note,
 *  e.g. "no usable placements" / "stopped at <hour>"). */
type FoldRender = (DayTasksResponse & { note?: string }) | null

function VariantColumn({ result, fold }: { result: LlmExperimentResult; fold: boolean }) {
  const [showRaw, setShowRaw] = useState(false)
  // Local selection so clicking a task card highlights it (there is no right
  // panel inside the modal to show its detail).
  const [selectedTask, setSelectedTask] = useState<string | null>(null)
  const meta = llmProvider(result.provider as LlmProviderId)
  const status = STATUS_META[result.status] ?? STATUS_META.pending

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
    <div className="flex flex-col shrink-0 rounded-xl overflow-hidden"
      style={{ width: fold ? 420 : 340, border: '1px solid var(--t-card-border)', background: 'var(--t-box)' }}>
      {/* header: provider + model chip + status badge */}
      <div className="flex items-center gap-2 px-3.5 py-2.5 border-b shrink-0" style={{ borderColor: 'var(--t-hair)' }}>
        <span className="mt-card-title truncate" style={{ color: 'var(--t-title)' }}>{meta.name}</span>
        {result.model !== '' && (
          <span className="font-mono truncate rounded px-1.5 py-0.5"
            style={{ fontSize: 10, background: 'var(--t-wrap)', color: 'var(--t-muted)' }}>
            {result.model}
          </span>
        )}
        <span className="ml-auto inline-flex items-center gap-1.5 shrink-0">
          {result.status === 'running' && (
            <span className="inline-block w-2 h-2 rounded-full animate-pulse" style={{ background: status.color }} />
          )}
          <span className="mt-body-sm" style={{ color: status.color, fontWeight: 700 }}>{status.label}</span>
        </span>
      </div>

      {/* metrics row */}
      <div className="flex items-center gap-3 px-3.5 py-1.5 border-b shrink-0"
        style={{ borderColor: 'var(--t-hair)', color: 'var(--t-faint)', fontSize: 10.5 }}>
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
        <p className="px-3.5 py-1.5 border-b shrink-0 mt-body-sm"
          style={{ borderColor: 'var(--t-hair)', color: 'var(--color-state-pending)', fontSize: 10.5 }}>
          {foldRender.note}
        </p>
      )}

      {/* body */}
      {timeline && foldRender ? (
        // The REAL dashboard timeline, fed this variant's simulated day.
        <div className="flex-1 min-h-0">
          <DayTaskColumn
            day={foldRender.day}
            isToday={false}
            selectedId={selectedTask}
            onSelect={d => setSelectedTask(d?.id ?? null)}
            tasks={foldRender.tasks}
          />
        </div>
      ) : (
        <div className="flex-1 min-h-0 overflow-y-auto nice-scroll px-3.5 py-3">
          {result.status === 'ok' && (
            <pre className="whitespace-pre-wrap break-words"
              style={{ font: '400 11.5px/1.55 var(--font-mono, ui-monospace)', color: 'var(--t-muted)', margin: 0 }}>
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
                  style={{ font: '400 10.5px/1.5 var(--font-mono, ui-monospace)', color: 'var(--t-faint)', margin: 0 }}>
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
