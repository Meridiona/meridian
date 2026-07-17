//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The dev-only "LLM Lab" modal: replay one pipeline prose stage across several
// provider/model variants and compare the outcomes side by side. Left rail =
// past runs (click to reopen); right pane = the RunComposer for a new run, or
// the ResultsGrid for the selected/active one. Rendered by MeridianTimelineShell
// ONLY when get_app_info reports the 'dev' channel; every backing command is
// additionally refused in release builds (commands/llm_lab.rs) - this surface
// does not exist for users. Plain hyphens in all copy.

'use client'

import { ModalShell } from '../ModalShell'
import { ResultsGrid } from './ResultsGrid'
import { RunComposer } from './RunComposer'
import { useLlmLab } from './useLlmLab'
import type { LlmExperimentSummary } from '@/lib/api-types'

const PROCESS_NAMES: Record<string, string> = {
  hour_report: 'Hour report',
  workstream_fold: 'Day-task fold',
  day_fold: 'Full-day fold',
  worklog_generate: 'Worklog draft',
}

export function LlmLabModal({ onClose }: { onClose: () => void }) {
  const { runs, detail, starting, error, run, openRun, closeRun } = useLlmLab()

  return (
    <ModalShell title="LLM Lab (dev only)" onClose={onClose} maxWidth={1180} scrollInside>
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

        {/* composer / results */}
        <div className="flex-1 min-w-0 min-h-0 flex flex-col p-5">
          {detail
            ? (
              <>
                <div className="flex items-baseline gap-2.5 pb-3 shrink-0">
                  <p className="mt-card-title" style={{ color: 'var(--t-title)' }}>
                    {PROCESS_NAMES[detail.process] ?? detail.process} · {detail.input_ref}
                  </p>
                  <span className="mt-body-sm" style={{ color: 'var(--t-faint)' }}>
                    {detail.status === 'running' ? 'running…' : detail.status} · run #{detail.id}
                  </span>
                </div>
                <ResultsGrid detail={detail} />
              </>
            )
            : <RunComposer starting={starting} error={error} onRun={run} />}
        </div>
      </div>
    </ModalShell>
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
