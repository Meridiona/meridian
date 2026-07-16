//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The LLM Lab's side-by-side comparison: one column per provider/model variant,
// each showing its outcome badge, timing/token metrics, and the rendered output
// (what the pipeline would have made of the raw answer), with a raw toggle.
// Columns scroll horizontally as a set; each column's output scrolls on its own.
// Dev-only surface - plain hyphens in all copy.

'use client'

import { useState } from 'react'
import type { LlmExperimentDetail, LlmExperimentResult } from '@/lib/api-types'
import { llmProvider, type LlmProviderId } from '@/lib/llm-providers'

const STATUS_META: Record<string, { label: string; color: string }> = {
  pending: { label: 'Queued', color: 'var(--t-faint)' },
  running: { label: 'Running…', color: 'var(--color-state-pending)' },
  ok: { label: 'OK', color: 'var(--color-state-approved)' },
  failed: { label: 'Failed', color: '#E11D48' },
  rate_limited: { label: 'Rate limited', color: '#E11D48' },
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
  return (
    <div className="flex-1 min-h-0 flex gap-3 overflow-x-auto nice-scroll" style={{ padding: 2 }}>
      {detail.results.map(r => (
        <VariantColumn key={r.variant_idx} result={r} />
      ))}
    </div>
  )
}

function VariantColumn({ result }: { result: LlmExperimentResult }) {
  const [showRaw, setShowRaw] = useState(false)
  const meta = llmProvider(result.provider as LlmProviderId)
  const status = STATUS_META[result.status] ?? STATUS_META.pending
  const body = showRaw ? result.output_text : result.output_rendered

  return (
    <div className="flex flex-col shrink-0 rounded-xl overflow-hidden"
      style={{ width: 340, border: '1px solid var(--t-card-border)', background: 'var(--t-box)' }}>
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
            {showRaw ? 'rendered' : 'raw'}
          </button>
        )}
      </div>

      {/* body */}
      <div className="flex-1 min-h-0 overflow-y-auto nice-scroll px-3.5 py-3">
        {result.status === 'ok' && (
          <pre className="whitespace-pre-wrap break-words"
            style={{ font: '400 11.5px/1.55 var(--font-mono, ui-monospace)', color: 'var(--t-muted)', margin: 0 }}>
            {body ?? '(no output recorded)'}
          </pre>
        )}
        {(result.status === 'failed' || result.status === 'rate_limited') && (
          <p className="mt-body-sm whitespace-pre-wrap break-words" style={{ color: '#E11D48' }}>
            {result.error ?? 'No error message recorded.'}
          </p>
        )}
        {(result.status === 'pending' || result.status === 'running') && (
          <p className="mt-body-sm" style={{ color: 'var(--t-faint)' }}>
            {result.status === 'pending'
              ? 'Waiting for its turn - variants run one at a time.'
              : 'The model is thinking - this can take a few minutes.'}
          </p>
        )}
      </div>
    </div>
  )
}
