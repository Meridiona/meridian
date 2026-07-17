//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The LLM Lab's task sidebar: when a task card is clicked in a fold variant's
// timeline (VariantBody -> DayTaskColumn), its breakdown renders HERE, beside the
// timeline. Read-only by design and DELIBERATELY not DayTaskDetailPanel - that
// panel drives useWorklog (get / generate / approve) against the PRODUCTION
// day_task_worklogs tables for a task id, and a fold's task ids are a model's
// SIMULATED day, not real rows. So the "when / what was done" section shows only
// what the fold result itself carries (the model's own per-task log).
//
// The "Draft with this model" action is on-demand and EPHEMERAL: it drafts THIS
// task's worklog with the variant currently shown (invoke -> draft_lab_worklog ->
// meridian llm-experiment draft-task), renders the answer here, and writes nothing
// anywhere. It fires a REAL, metered completion, so it carries a free/local
// caution - never auto-run. Dev-only surface - plain hyphens in all copy.

'use client'

import { useState } from 'react'
import { fmtDur } from '@/components/atoms'
import { invoke } from '@/lib/bridge'
import { clockLabel } from '../dayTaskLayout'
import { Bullets, Field } from '../dayTaskKit'
import type { DayTaskDetail } from '../DayTaskDetailPanel'

/** The worklog_generate answer shape (mirrors prompts::worklog_generate_schema).
 *  Every field is optional here because this renders a RAW model answer that may
 *  be partial or malformed - we fall back to the raw text when it doesn't parse. */
interface WorklogDraft {
  matches?: { task_key: string; confidence: number }[]
  propose?: { issue_type: string; title: string; description: string } | null
  update?: { summary?: string; sections?: { heading: string; points: string[] }[] }
}

export function LabTaskSidebar({ detail, variantToken, onClose }: {
  detail: DayTaskDetail
  // The variant token to draft with (e.g. "local", "custom:nvidia"). Threaded
  // from RunView so the draft uses whichever variant the timeline is showing.
  variantToken: string
  onClose: () => void
}) {
  const { title, hue, minutes, segments, summary, footLo, footHi, day } = detail
  const range = segments.length > 0 ? `${clockLabel(footLo)} - ${clockLabel(footHi)}` : ''
  const sittings = segments.length

  const [drafting, setDrafting] = useState(false)
  const [draft, setDraft] = useState<string | null>(null)
  const [draftErr, setDraftErr] = useState<string | null>(null)
  const [showRaw, setShowRaw] = useState(false)

  async function onDraft() {
    setDrafting(true)
    setDraft(null)
    setDraftErr(null)
    try {
      const out = await invoke<string>('draft_lab_worklog', {
        body: { day, variant: variantToken, task: { title, summary, minutes } },
      })
      setDraft(out)
    } catch (e) {
      setDraftErr(e instanceof Error ? e.message : String(e))
    } finally {
      setDrafting(false)
    }
  }

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

        {/* On-demand draft: one real, metered call with the shown variant. */}
        <div className="pt-1 border-t" style={{ borderColor: 'var(--t-hair)' }}>
          <div className="pt-3">
            <button onClick={onDraft} disabled={drafting}
              className="w-full rounded-lg px-3 py-2"
              style={{
                border: '1px solid var(--t-ctrl-border)',
                background: drafting ? 'var(--t-wrap)' : 'var(--t-ctrl)',
                color: 'var(--t-title)', cursor: drafting ? 'default' : 'pointer',
                font: '700 12px var(--font-sans)',
              }}>
              {drafting ? 'Drafting…' : 'Draft worklog with this model'}
            </button>
            <p className="mt-body-sm mt-1.5" style={{ color: 'var(--t-faint)', fontSize: 10.5, lineHeight: 1.45 }}>
              Sends one real request to this variant - run it only on free or local endpoints.
            </p>
          </div>

          {draftErr && (
            <p className="mt-body-sm mt-3 whitespace-pre-wrap break-words" style={{ color: '#E11D48' }}>
              {draftErr}
            </p>
          )}

          {draft !== null && (
            <div className="mt-3">
              <div className="flex items-center justify-between mb-1">
                <p className="mt-label" style={{ color: 'var(--t-faint)' }}>DRAFTED WORKLOG</p>
                <button onClick={() => setShowRaw(v => !v)} className="rounded px-1.5 py-0.5"
                  style={{
                    border: '1px solid var(--t-ctrl-border)', fontSize: 10, cursor: 'pointer',
                    background: showRaw ? 'var(--t-wrap)' : 'var(--t-ctrl)', color: 'var(--t-muted)',
                  }}>
                  {showRaw ? 'formatted' : 'raw'}
                </button>
              </div>
              <DraftBody raw={draft} showRaw={showRaw} accent={hue} />
              <p className="mt-body-sm mt-2" style={{ color: 'var(--t-faint)', fontSize: 10.5 }}>
                Not posted anywhere - a Lab draft for comparison only.
              </p>
            </div>
          )}
        </div>
      </div>
    </div>
  )
}

/** Render the model's raw worklog_generate answer: the drafted update prose when
 *  it parses to the expected shape, else the raw text. `raw` mode always shows the
 *  raw answer verbatim. */
function DraftBody({ raw, showRaw, accent }: { raw: string; showRaw: boolean; accent: string }) {
  let parsed: WorklogDraft | null = null
  if (!showRaw) {
    try {
      parsed = JSON.parse(raw) as WorklogDraft
    } catch {
      parsed = null
    }
  }

  if (showRaw || !parsed?.update) {
    return (
      <pre className="whitespace-pre-wrap break-words"
        style={{ font: '400 11px/1.5 var(--font-mono, ui-monospace)', color: 'var(--t-muted)', margin: 0 }}>
        {raw}
      </pre>
    )
  }

  const { matches, propose, update } = parsed
  const target = matches && matches.length > 0
    ? `Posts to ${matches.map(m => m.task_key).join(', ')}`
    : propose
      ? `Proposes a new ticket: [${propose.issue_type}] ${propose.title}`
      : 'No ticket match - unposted note'

  return (
    <div className="space-y-2">
      <p className="mt-body-sm" style={{ color: 'var(--t-faint)', fontSize: 10.5 }}>{target}</p>
      {update?.summary && (
        <p className="mt-body-sm" style={{ color: 'var(--t-muted)', lineHeight: 1.5 }}>{update.summary}</p>
      )}
      {update?.sections?.map((s, i) => (
        <div key={i}>
          <p className="mt-body-sm mt-1" style={{ color: 'var(--t-title)', fontWeight: 700, fontSize: 11.5 }}>
            {s.heading}
          </p>
          <Bullets items={s.points} accent={accent} size={11.5} />
        </div>
      ))}
    </div>
  )
}
