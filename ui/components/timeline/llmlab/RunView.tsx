//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// One LLM Lab run, shown as a single timeline with a variant switcher. The run
// fanned a prose stage across N provider/model variants; instead of N columns
// side by side, this shows ONE variant full-width (VariantBody) with a tab strip
// to swap between them - so a fold variant's day fills the pane as the real
// dashboard timeline. Clicking a task card opens the run's shared LabTaskSidebar
// beside the timeline. Mounted by LlmLabScreen (keyed by run id). Dev-only
// surface - plain hyphens in all copy.

'use client'

import { useEffect, useMemo, useState } from 'react'
import type { LlmExperimentDetail, LlmExperimentResult } from '@/lib/api-types'
import { llmProvider, type LlmProviderId } from '@/lib/llm-providers'
import type { DayTaskDetail } from '../DayTaskDetailPanel'
import { VariantBody, STATUS_META, isFoldProcess } from './VariantBody'
import { LabTaskSidebar } from './LabTaskSidebar'

/** The variant token the daemon understands (matches the composer / CLI): a
 *  custom endpoint is `custom:<id>`, an explicit model is `provider:model`, and a
 *  bare provider is just its id. Threaded to the draft button so it drafts with
 *  the variant currently shown. */
function variantToken(r: LlmExperimentResult): string {
  if (r.provider === 'custom') return `custom:${r.model}`
  return r.model ? `${r.provider}:${r.model}` : r.provider
}

/** Wire process -> display name. Also used by LlmLabScreen's past-runs rail. */
export const PROCESS_NAMES: Record<string, string> = {
  hour_report: 'Hour report',
  workstream_fold: 'Day-task fold',
  day_fold: 'Full-day fold',
  worklog_generate: 'Worklog draft',
}

export function RunView({ detail }: { detail: LlmExperimentDetail }) {
  const fold = isFoldProcess(detail.process)
  const results = detail.results

  // The first ok variant, else the first. Recomputed as the run polls, so it
  // tracks completions rather than being frozen at mount.
  const firstOk = useMemo(() => {
    const i = results.findIndex(r => r.status === 'ok')
    return i >= 0 ? i : 0
  }, [results])

  const [selectedIdx, setSelectedIdx] = useState(firstOk)
  const [selectedTask, setSelectedTask] = useState<DayTaskDetail | null>(null)
  // Whether the user has clicked a switcher tab. Until they do, the view
  // auto-follows the first variant to finish; the view is keyed by run id, so a
  // new run starts untouched.
  const [touched, setTouched] = useState(false)

  // Auto-follow the first completion: while untouched, if the shown variant is
  // not itself ok and SOME variant now is, snap to the first ok one. This lands a
  // still-running run on whichever model finishes first, instead of sitting on a
  // pending/failed variant. Once the shown variant is ok (or the user clicks) it
  // stays put - no jumping back to a lower-index variant that finishes later.
  useEffect(() => {
    if (touched) return
    if (results[selectedIdx]?.status !== 'ok' && results.some(r => r.status === 'ok')) {
      setSelectedIdx(firstOk)
    }
  }, [touched, results, selectedIdx, firstOk])

  // The results array grows/settles as the run polls; keep the index in range.
  useEffect(() => {
    if (selectedIdx >= results.length) setSelectedIdx(0)
  }, [results.length, selectedIdx])

  const selected = results[selectedIdx]

  // A manual pick freezes the choice (stops auto-follow) and clears the task
  // selection - task ids (T1, T2) are per-variant (each model builds its OWN
  // day), so a stale id must not carry across a switch.
  function pickVariant(idx: number) {
    setSelectedIdx(idx)
    setSelectedTask(null)
    setTouched(true)
  }

  return (
    <div className="flex-1 min-h-0 flex flex-col">
      {/* run header */}
      <div className="flex items-baseline gap-2.5 px-5 pt-4 pb-3 shrink-0">
        <p className="mt-card-title" style={{ color: 'var(--t-title)' }}>
          {PROCESS_NAMES[detail.process] ?? detail.process} · {detail.input_ref}
        </p>
        <span className="mt-body-sm" style={{ color: 'var(--t-faint)' }}>
          {detail.status === 'running' ? 'running…' : detail.status} · run #{detail.id}
        </span>
      </div>

      {/* variant switcher */}
      <div className="flex items-center gap-2 px-5 pb-3 shrink-0 overflow-x-auto nice-scroll">
        {results.map((r, idx) => {
          const meta = llmProvider(r.provider as LlmProviderId)
          const status = STATUS_META[r.status] ?? STATUS_META.pending
          const active = idx === selectedIdx
          return (
            <button key={r.variant_idx} onClick={() => pickVariant(idx)}
              className="inline-flex items-center gap-2 rounded-full px-3 py-1.5 shrink-0"
              style={{
                border: `1px solid ${active ? 'var(--btn-primary-bg)' : 'var(--t-card-border)'}`,
                background: active ? 'var(--t-wrap)' : 'var(--t-box)',
                cursor: 'pointer',
              }}>
              <span className="inline-block w-2 h-2 rounded-full shrink-0" style={{ background: status.color }} />
              <span className="mt-card-title" style={{ color: active ? 'var(--t-title)' : 'var(--t-muted)', fontSize: 12 }}>
                {meta.name}
              </span>
              {r.model !== '' && (
                <span className="font-mono rounded px-1 py-0.5"
                  style={{ fontSize: 9.5, background: 'var(--t-wrap)', color: 'var(--t-faint)' }}>
                  {r.model}
                </span>
              )}
            </button>
          )
        })}
      </div>

      {/* selected variant, full-width, + the run's shared task sidebar */}
      <div className="flex-1 min-h-0 flex border-t" style={{ borderColor: 'var(--t-hair)' }}>
        {selected ? (
          <VariantBody
            key={selected.variant_idx}
            result={selected}
            fold={fold}
            selectedTask={selectedTask}
            onSelectTask={setSelectedTask}
          />
        ) : (
          <p className="flex-1 mt-body-sm px-5 py-4" style={{ color: 'var(--t-faint)' }}>
            This run has no variants.
          </p>
        )}
        {fold && selectedTask && selected && (
          <LabTaskSidebar
            key={selectedTask.id}
            detail={selectedTask}
            variantToken={variantToken(selected)}
            onClose={() => setSelectedTask(null)}
          />
        )}
      </div>
    </div>
  )
}
