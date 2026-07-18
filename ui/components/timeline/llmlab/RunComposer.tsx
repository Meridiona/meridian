//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The LLM Lab's "new run" form: pick a process, the past input to replay (an
// hour for the report/fold processes, a day-task for worklog-generate), and the
// provider/model variants to fan it across. Variant cards reuse the provider
// list (@/lib/llm-providers) and the live install/connectivity detection hook
// (@/components/LlmProviderPicker) so an uninstalled CLI can't be picked.
// Dev-only surface - plain hyphens in all copy.

'use client'

import { useEffect, useMemo, useState } from 'react'
import { load } from '@/lib/bridge'
import type { DayTasksResponse, LlmExperimentProcess, RunLlmExperimentBody } from '@/lib/api-types'
import { LLM_PROVIDERS, customVariantId, rungLabel, type LlmProviderId } from '@/lib/llm-providers'
import { useLlmProviderDetection } from '@/components/LlmProviderPicker'
import { useCustomProviders } from '@/components/CustomProviders'
import { ModelMultiSelect, ModelUnsupportedNote } from '@/components/ModelPicker'
import { dayString } from '../types'

const PROCESSES: { id: LlmExperimentProcess; name: string; hint: string }[] = [
  { id: 'hour_report', name: 'Hour report', hint: 'The hourly activity summary - replays a distilled past hour.' },
  { id: 'workstream_fold', name: 'Day-task fold', hint: 'Folding an hour’s report into the day’s tasks - each model’s resulting day renders as the real timeline. Uses the day’s CURRENT prior tasks, not the fold-time state.' },
  { id: 'day_fold', name: 'Full-day fold', hint: 'Replays EVERY processed hour of the day, in order - each model builds its own day from scratch and the final timelines compare side by side. One request per hour per model.' },
  { id: 'worklog_generate', name: 'Worklog draft', hint: 'Match a day-task to a ticket + draft the status update.' },
]

export function RunComposer({ starting, error, onRun }: {
  starting: boolean
  error: string | null
  onRun: (body: RunLlmExperimentBody) => void
}) {
  const [process, setProcess] = useState<LlmExperimentProcess>('hour_report')
  const [day, setDay] = useState<string>(dayString(0))
  const [hour, setHour] = useState<number>(new Date().getHours() > 0 ? new Date().getHours() - 1 : 0)
  const [taskId, setTaskId] = useState<string>('')
  const [picked, setPicked] = useState<Set<LlmProviderId>>(new Set(['claude']))
  const [models, setModels] = useState<Partial<Record<LlmProviderId, string>>>({})
  // Custom endpoints are picked by id, separately from the built-in providers - they aren't
  // LlmProviderIds and the Lab runs ANY measured endpoint, so there's no install/eligibility
  // gate on them here (unlike the production picker).
  const [pickedCustom, setPickedCustom] = useState<Set<string>>(new Set())

  const { status, scanning } = useLlmProviderDetection()
  const custom = useCustomProviders()

  // Day-task options for worklog-generate: the picked day's cards.
  const [dayTasks, setDayTasks] = useState<DayTasksResponse | null>(null)
  useEffect(() => {
    if (process !== 'worklog_generate') return
    let live = true
    load<DayTasksResponse>('/api/day-tasks', 'get_day_tasks', { day })
      .then(r => { if (live) { setDayTasks(r); setTaskId(t => r.tasks.some(x => x.id === t) ? t : (r.tasks[0]?.id ?? '')) } })
      .catch(() => { if (live) setDayTasks(null) })
    return () => { live = false }
  }, [process, day])

  function toggle(id: LlmProviderId) {
    setPicked(prev => {
      const next = new Set(prev)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })
  }

  function toggleCustom(id: string) {
    setPickedCustom(prev => {
      const next = new Set(prev)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })
  }

  // One variant per provider by default; a comma-separated model list fans the
  // SAME provider out once per model ("gpt-5.3, gpt-5.1-mini" -> two columns), so
  // models within a provider compare exactly like providers do. Custom endpoints
  // append as `custom:<id>` tokens - their model is fixed by the endpoint, so no
  // model-override fan-out.
  const variants = useMemo(() => ([
    ...LLM_PROVIDERS.filter(p => picked.has(p.id)).flatMap(p => {
      // A provider whose backend discards the model (copilot) never fans out, even if a
      // stale string is still held in state - a `copilot:<model>` column would promise a
      // comparison the run can't actually make.
      const overrides = p.supportsModelOverride
        ? (models[p.id] ?? '').split(',').map(m => m.trim()).filter(Boolean)
        : []
      return overrides.length ? overrides.map(m => `${p.id}:${m}`) : [p.id]
    }),
    ...custom.providers.filter(c => pickedCustom.has(c.id)).map(c => customVariantId(c.id)),
  ]), [picked, models, pickedCustom, custom.providers])

  const hourLabel = `${day}T${String(hour).padStart(2, '0')}`
  const inputOk = process === 'worklog_generate' ? taskId !== '' : true
  const canRun = !starting && variants.length > 0 && inputOk

  function submit() {
    const body: RunLlmExperimentBody =
      process === 'worklog_generate' ? { process, day, task_id: taskId, variants }
      : process === 'day_fold' ? { process, day, variants }
      : { process, hour: hourLabel, variants }
    onRun(body)
  }

  const processMeta = PROCESSES.find(p => p.id === process) ?? PROCESSES[0]

  return (
    <div className="flex-1 min-h-0 overflow-y-auto nice-scroll pr-1">
      {/* process */}
      <p className="mt-label" style={{ color: 'var(--t-faint)', marginBottom: 8 }}>PROCESS</p>
      <div className="flex gap-1.5">
        {PROCESSES.map(p => (
          <button key={p.id} onClick={() => setProcess(p.id)}
            className="rounded-full px-3 py-1.5"
            style={{
              border: '1px solid var(--t-ctrl-border)', cursor: 'pointer',
              background: p.id === process ? 'var(--btn-primary-bg)' : 'var(--t-ctrl)',
              color: p.id === process ? '#fff' : 'var(--t-muted)',
              font: '600 12px var(--font-sans)',
            }}>
            {p.name}
          </button>
        ))}
      </div>
      <p className="mt-body-sm mt-1.5" style={{ color: 'var(--t-faint)' }}>{processMeta.hint}</p>

      {/* input */}
      <p className="mt-label mt-5" style={{ color: 'var(--t-faint)', marginBottom: 8 }}>REPLAY INPUT</p>
      <div className="flex items-center gap-2">
        <input type="date" value={day} max={dayString(0)} onChange={e => setDay(e.target.value)}
          className="rounded-lg px-2.5 py-1.5 bg-ctrl"
          style={{ border: '1px solid var(--t-ctrl-border)', color: 'var(--t-title)', font: '500 12px var(--font-sans)' }} />
        {process === 'day_fold' ? null : process !== 'worklog_generate' ? (
          <select value={hour} onChange={e => setHour(Number(e.target.value))}
            className="rounded-lg px-2.5 py-1.5 bg-ctrl"
            style={{ border: '1px solid var(--t-ctrl-border)', color: 'var(--t-title)', font: '500 12px var(--font-sans)' }}>
            {Array.from({ length: 24 }, (_, h) => (
              <option key={h} value={h}>{String(h).padStart(2, '0')}:00 - {String(h + 1).padStart(2, '0')}:00</option>
            ))}
          </select>
        ) : (
          <select value={taskId} onChange={e => setTaskId(e.target.value)}
            className="rounded-lg px-2.5 py-1.5 bg-ctrl min-w-0"
            style={{ border: '1px solid var(--t-ctrl-border)', color: 'var(--t-title)', font: '500 12px var(--font-sans)', maxWidth: 320 }}>
            {(dayTasks?.tasks ?? []).map(t => (
              <option key={t.id} value={t.id}>{t.id} - {t.title}</option>
            ))}
            {(dayTasks?.tasks ?? []).length === 0 && <option value="">no day-tasks on this day</option>}
          </select>
        )}
      </div>
      {process === 'day_fold' ? (
        <p className="mt-body-sm mt-1.5" style={{ color: 'var(--t-faint)' }}>
          Every hour of this day with a stored activity report is folded in order. Expect a few
          minutes per model - and one request per processed hour per model.
        </p>
      ) : process !== 'worklog_generate' && (
        <p className="mt-body-sm mt-1.5" style={{ color: 'var(--t-faint)' }}>
          Only hours the pipeline already processed can be replayed - an hour with no stored
          {process === 'hour_report' ? ' distilled text' : ' activity report'} will refuse with a clear message.
        </p>
      )}

      {/* variants */}
      <p className="mt-label mt-5" style={{ color: 'var(--t-faint)', marginBottom: 8 }}>VARIANTS</p>
      <div className="grid grid-cols-2 gap-2">
        {LLM_PROVIDERS.map(p => {
          const st = status[p.id]
          const installable = st?.installed ?? true
          const on = picked.has(p.id)
          return (
            <div key={p.id} className="rounded-xl px-3 py-2.5"
              style={{
                border: `1px solid ${on ? 'var(--btn-primary-bg)' : 'var(--t-card-border)'}`,
                background: 'var(--t-box)', opacity: installable ? 1 : 0.55,
              }}>
              <label className="flex items-center gap-2" style={{ cursor: installable ? 'pointer' : 'not-allowed' }}>
                <input type="checkbox" checked={on} disabled={!installable} onChange={() => toggle(p.id)} />
                <span className="mt-card-title" style={{ color: 'var(--t-title)' }}>{p.name}</span>
                <span className="ml-auto mt-body-sm" style={{ color: 'var(--t-faint)', fontSize: 10.5 }}>
                  {scanning && !st ? 'scanning…'
                    : installable ? 'installed' : 'not installed'}
                </span>
              </label>
              {/* Tick curated models and/or type your own; both feed the same comma list, so
                  the variant assembly above is unchanged. copilot takes no model at all, so
                  it gets a note rather than a control that would be silently discarded. */}
              {on && (p.supportsModelOverride ? (
                <ModelMultiSelect
                  value={models[p.id] ?? ''}
                  onChange={next => setModels(m => ({ ...m, [p.id]: next }))}
                  models={p.models}
                />
              ) : (
                <div className="mt-2">
                  <ModelUnsupportedNote providerName={p.name} />
                </div>
              ))}
            </div>
          )
        })}

        {/* The user's own endpoints. Unlike the production picker, the Lab runs ANY measured
            endpoint - even one whose rung is too weak for production - so these have no
            eligibility gate; the rung is shown for context, not as a lock. */}
        {custom.providers.map(c => {
          const on = pickedCustom.has(c.id)
          return (
            <div key={c.id} className="rounded-xl px-3 py-2.5"
              style={{
                border: `1px solid ${on ? 'var(--btn-primary-bg)' : 'var(--t-card-border)'}`,
                background: 'var(--t-box)',
              }}>
              <label className="flex items-center gap-2" style={{ cursor: 'pointer' }}>
                <input type="checkbox" checked={on} onChange={() => toggleCustom(c.id)} />
                <span className="mt-card-title truncate" style={{ color: 'var(--t-title)' }}>{c.name}</span>
                <span className="ml-auto mt-body-sm shrink-0" style={{ color: 'var(--t-faint)', fontSize: 10.5 }}>
                  {rungLabel(c.effective_rung)}
                </span>
              </label>
              {on && (
                <p className="mt-1.5 font-mono truncate" style={{ color: 'var(--t-faint)', fontSize: 10.5 }}>
                  {c.model}
                </p>
              )}
            </div>
          )
        })}
      </div>

      {/* run */}
      <div className="flex items-center gap-3 mt-5">
        <button onClick={submit} disabled={!canRun}
          className="rounded-xl px-5 py-2.5"
          style={{
            border: 'none', background: canRun ? 'var(--btn-primary-bg)' : 'var(--t-ctrl)',
            color: canRun ? '#fff' : 'var(--t-faint)', font: '700 12.5px var(--font-sans)',
            cursor: canRun ? 'pointer' : 'default',
          }}>
          {starting ? 'Starting…' : `Run ${variants.length || ''} variant${variants.length === 1 ? '' : 's'}`}
        </button>
        <span className="mt-body-sm" style={{ color: 'var(--t-faint)' }}>
          Variants run one at a time - each spends one real request on that provider.
        </span>
      </div>
      {error && <p className="mt-body-sm mt-3" style={{ color: '#E11D48' }}>{error}</p>}
    </div>
  )
}
