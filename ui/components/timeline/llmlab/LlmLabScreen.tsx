//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The dev-only "LLM Lab" surface: replay one pipeline prose stage across several
// provider/model variants and compare the outcomes. A FULL-SCREEN panel (its own
// dashboard, not a modal) - left rail = past runs (click to reopen); right =
// the RunComposer for a new run, or RunView for the selected/active one (a single
// timeline with a variant switcher + task sidebar). Rendered by
// MeridianTimelineShell ONLY when get_app_info reports the 'dev' channel; every
// backing command is additionally refused in release builds (commands/llm_lab.rs)
// - this surface does not exist for users. Plain hyphens in all copy.

'use client'

import { useEffect } from 'react'
import { RunComposer } from './RunComposer'
import { RunView, PROCESS_NAMES } from './RunView'
import { useLlmLab } from './useLlmLab'
import type { LlmExperimentSummary } from '@/lib/api-types'

export function LlmLabScreen({ onClose }: { onClose: () => void }) {
  const { runs, detail, starting, error, run, openRun, closeRun } = useLlmLab()

  // Escape closes the surface (matches the wrapper-modal convention it replaces).
  useEffect(() => {
    function onKey(e: KeyboardEvent) { if (e.key === 'Escape') onClose() }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose])

  return (
    <div className="absolute inset-0 z-40 flex flex-col bg-panel rise">
      {/* header */}
      <div className="flex items-center justify-between px-6 py-4 border-b shrink-0" style={{ borderColor: 'var(--t-hair)' }}>
        <p className="mt-modal-title text-title">LLM Lab (dev only)</p>
        <button onClick={onClose} aria-label="Close"
          className="inline-flex items-center justify-center rounded-full bg-wrap"
          style={{ width: 30, height: 30, color: 'var(--t-muted)' }}>
          <span className="text-[17px] leading-none">×</span>
        </button>
      </div>

      {/* body: past-runs rail | composer or run */}
      <div className="flex flex-1 min-h-0">
        {/* past runs rail */}
        <div className="shrink-0 border-r flex flex-col min-h-0" style={{ width: 250, borderColor: 'var(--t-hair)' }}>
          <div className="flex items-center justify-between px-4 py-3 shrink-0">
            <p className="mt-label" style={{ color: 'var(--t-faint)' }}>PAST RUNS</p>
            <button onClick={closeRun}
              className="rounded-full px-2.5 py-1"
              style={{
                border: '1px solid var(--t-ctrl-border)', cursor: 'pointer',
                background: detail === null ? 'var(--btn-primary-bg)' : 'var(--t-ctrl)',
                color: detail === null ? '#fff' : 'var(--t-muted)',
                font: '700 11px var(--font-sans)',
              }}>
              + New run
            </button>
          </div>
          <div className="flex-1 min-h-0 overflow-y-auto nice-scroll px-3 pb-3">
            {runs.length === 0 && (
              <p className="mt-body-sm px-1" style={{ color: 'var(--t-faint)' }}>
                No runs yet - compose one on the right.
              </p>
            )}
            {runs.map(r => (
              <RunRow key={r.id} run={r} active={detail?.id === r.id} onClick={() => openRun(r.id)} />
            ))}
          </div>
        </div>

        {/* composer / run */}
        <div className="flex-1 min-w-0 min-h-0 flex flex-col">
          {detail
            ? <RunView key={detail.id} detail={detail} />
            : (
              <div className="flex-1 min-h-0 overflow-y-auto nice-scroll p-5">
                <RunComposer starting={starting} error={error} onRun={run} />
              </div>
            )}
        </div>
      </div>
    </div>
  )
}

function RunRow({ run, active, onClick }: {
  run: LlmExperimentSummary
  active: boolean
  onClick: () => void
}) {
  const statusColor = run.status === 'done' ? 'var(--color-state-approved)'
    : run.status === 'failed' ? '#E11D48' : 'var(--color-state-pending)'
  return (
    <button onClick={onClick} className="w-full text-left rounded-xl px-3 py-2.5 mb-1.5"
      style={{
        border: `1px solid ${active ? 'var(--btn-primary-bg)' : 'var(--t-card-border)'}`,
        background: 'var(--t-box)', cursor: 'pointer',
      }}>
      <div className="flex items-center gap-2">
        <span className="inline-block w-2 h-2 rounded-full shrink-0" style={{ background: statusColor }} />
        <span className="mt-card-title truncate" style={{ color: 'var(--t-title)', fontSize: 12 }}>
          {PROCESS_NAMES[run.process] ?? run.process}
        </span>
        <span className="ml-auto mt-body-sm shrink-0" style={{ color: 'var(--t-faint)', fontSize: 10.5 }}>
          {run.n_done}/{run.n_variants}
        </span>
      </div>
      <p className="mt-body-sm truncate mt-0.5" style={{ color: 'var(--t-faint)', fontSize: 11 }}>
        {run.input_ref}
      </p>
    </button>
  )
}
