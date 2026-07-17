//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The daily summary: one screen, composed by the model, about how a day went.
//
// ONE SCREEN, NO PAGE SCROLL. That is the whole design constraint. The panel grid
// flexes to the space left over after the prose, and the panel count is capped
// server-side (2-4), so this never grows a scrollbar. If something must give, it is
// the text — the charts are the point.
//
// Nothing here decides what to show. The narrative, the insights, which charts, how
// many, and what form each takes all come from the model; this lays them out. The
// specs carry FORM only, so the real rows are fetched separately
// (get_day_summary_data) and injected by VegaPanel at render.
//
// Clicking a workstream opens the SAME DayTaskDetailPanel the timeline uses, in a
// dialog — so generate/approve/retarget/dismiss all work here with no new worklog
// code (useWorklog is keyed by (day, taskId) in a module store, so a generate
// started here survives closing the dialog).

'use client'

import { useCallback, useEffect, useMemo, useState } from 'react'
import { AnimatePresence, motion } from 'framer-motion'
import { load, invoke } from '@/lib/bridge'
import type { DaySummary, DaySummaryData, DayTask, DayTasksResponse } from '@/lib/api-types'
import { DayTaskDetailPanel, type DayTaskDetail } from '@/components/timeline/DayTaskDetailPanel'
import { taskHue } from '@/components/timeline/dayTaskKit'
import { hhmmToMin } from '@/components/timeline/dayTaskLayout'
import { formatDayLabel } from '@/components/timeline/types'
import type { SettingsSection } from '@/components/timeline/settings/types'
import { VegaPanel } from './VegaPanel'

const API = '/api/day-summary' // vestigial route label the bridge wants (Tauri-only now)

/** Build the timeline's own detail payload from a DayTask, so the reused panel gets
 *  exactly the shape it already expects. Mirrors DayTaskColumn's construction. */
function detailOf(task: DayTask, idx: number, day: string): DayTaskDetail {
  const segments = (task.segments ?? [])
    .map(s => ({ startMin: hhmmToMin(s.start), endMin: hhmmToMin(s.end) }))
    .filter((s): s is { startMin: number; endMin: number } => s.startMin !== null && s.endMin !== null)
  const footLo = segments.length ? Math.min(...segments.map(s => s.startMin)) : 0
  const footHi = segments.length ? Math.max(...segments.map(s => s.endMin)) : 0
  return {
    id: task.id,
    day,
    title: task.title,
    minutes: task.minutes,
    hue: taskHue(task.id, idx),
    segments,
    summary: task.summary ?? [],
    footLo,
    footHi,
    linkedTicket: task.linked_ticket,
  }
}

function NavBtn({ glyph, label, onClick, disabled }: {
  glyph: string; label: string; onClick: () => void; disabled?: boolean
}) {
  return (
    <button onClick={onClick} disabled={disabled} aria-label={label} title={label}
      className="inline-flex items-center justify-center rounded-full transition-opacity disabled:opacity-30"
      style={{ width: 26, height: 26, color: 'var(--t-muted)', border: '1px solid var(--t-hair)' }}>
      <span className="text-[13px] leading-none">{glyph}</span>
    </button>
  )
}

export function DaySummaryOverlay({ day, isToday, onShiftDay, onClose, onOpenSettings, onOpenTask }: {
  day: string
  isToday: boolean
  onShiftDay: (delta: number) => void
  onClose: () => void
  onOpenSettings: (section?: SettingsSection) => void
  onOpenTask: (key: string, title?: string) => void
}) {
  const [summary, setSummary] = useState<DaySummary | null>(null)
  const [data, setData] = useState<DaySummaryData>({})
  const [tasks, setTasks] = useState<DayTask[]>([])
  const [loading, setLoading] = useState(true)
  const [generating, setGenerating] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [selected, setSelected] = useState<DayTaskDetail | null>(null)

  // Re-read everything on a day change. The summary and its data must come from
  // the same day or a chart would render one day's rows under another's title.
  useEffect(() => {
    let cancelled = false
    setLoading(true)
    setSelected(null)
    setError(null)
    Promise.allSettled([
      load<DaySummary | null>(API, 'get_day_summary', { day }),
      load<DaySummaryData>(API, 'get_day_summary_data', { day }),
      load<DayTasksResponse>('/api/day-tasks', 'get_day_tasks', { day }),
    ]).then(([s, d, t]) => {
      if (cancelled) return
      setSummary(s.status === 'fulfilled' ? s.value : null)
      setData(d.status === 'fulfilled' ? (d.value ?? {}) : {})
      setTasks(t.status === 'fulfilled' ? (t.value?.tasks ?? []) : [])
      setLoading(false)
    })
    return () => { cancelled = true }
  }, [day])

  const generate = useCallback(async () => {
    setGenerating(true)
    setError(null)
    try {
      const s = await invoke<DaySummary>('generate_day_summary', { day })
      setSummary(s)
      // Re-read the rows alongside: a regenerate can be minutes after the last
      // fold, and the panels must bind to what is true now.
      await load<DaySummaryData>(API, 'get_day_summary_data', { day })
        .then(setData)
        .catch(() => {})
    } catch (e) {
      setError(e instanceof Error ? e.message : typeof e === 'string' ? e : 'Could not compose the summary')
    } finally {
      setGenerating(false)
    }
  }, [day])

  const dayLabel = isToday ? 'Today' : formatDayLabel(day)
  const hasWork = tasks.length > 0

  // The workstreams, in the timeline's own order and colours, so a task looks the
  // same here as it does there.
  const details = useMemo(() => tasks.map((t, i) => detailOf(t, i, day)), [tasks, day])

  return (
    <div className="absolute inset-0 z-50 flex flex-col rise" style={{ background: 'var(--win-bg)' }}>
      {/* Header */}
      <div className="shrink-0 flex items-center justify-between px-5 py-3 border-b"
        style={{ borderColor: 'var(--t-hair)' }}>
        <div className="flex items-center gap-2.5 min-w-0">
          <NavBtn glyph="‹" label="Previous day" onClick={() => onShiftDay(-1)} />
          <div className="min-w-0">
            <p className="mt-title truncate" style={{ color: 'var(--t-title)' }}>
              {dayLabel}
            </p>
            {summary && (
              <p className="mt-body-sm truncate" style={{ color: 'var(--t-faint-2)' }}>
                {summary.fallback
                  ? 'A plain view of the day - the AI could not compose one'
                  : `Composed by ${summary.model || summary.provider}`}
              </p>
            )}
          </div>
          <NavBtn glyph="›" label="Next day" onClick={() => onShiftDay(1)} disabled={isToday} />
        </div>

        <div className="flex items-center gap-2 shrink-0">
          {hasWork && (
            <button onClick={generate} disabled={generating}
              className="rounded-full px-3.5 py-1.5 mt-label transition-opacity disabled:opacity-50"
              style={{ background: 'var(--accent)', color: '#fff' }}>
              {generating ? 'Composing…' : summary ? 'Regenerate' : 'Generate summary'}
            </button>
          )}
          <button onClick={onClose} aria-label="Close"
            className="inline-flex items-center justify-center rounded-full"
            style={{ width: 28, height: 28, color: 'var(--t-muted)', border: '1px solid var(--t-hair)' }}>
            <span className="text-[16px] leading-none">×</span>
          </button>
        </div>
      </div>

      {/* Body - the one screen. min-h-0 all the way down is what keeps the grid
          inside the viewport instead of pushing a page scrollbar. */}
      <div className="flex-1 min-h-0 flex flex-col px-5 py-4 gap-3">
        {loading ? (
          <Centered text="Reading your day…" />
        ) : !hasWork ? (
          <Centered text={`Nothing was tracked on ${dayLabel.toLowerCase()}, so there is no day to review.`} />
        ) : !summary ? (
          <Centered
            text={generating
              ? 'Reading the whole day and deciding what is worth showing. This takes about a minute.'
              : 'Generate a summary to see how this day actually went.'}
          />
        ) : (
          <>
            {/* Prose - deliberately small. The charts are the point. */}
            {(summary.narrative || summary.insights.length > 0) && (
              <div className="shrink-0 flex gap-6 items-start">
                {summary.narrative && (
                  <p className="mt-body flex-1 min-w-0" style={{ color: 'var(--t-title)', maxWidth: '62ch' }}>
                    {summary.narrative}
                  </p>
                )}
                {summary.insights.length > 0 && (
                  <ul className="shrink-0 flex flex-col gap-1" style={{ maxWidth: 340 }}>
                    {summary.insights.map((i, n) => (
                      <li key={n} className="flex gap-2 items-start">
                        <span className="shrink-0 rounded-full" style={{
                          width: 5, height: 5, background: 'var(--accent)', marginTop: 6,
                        }} />
                        <span className="mt-body-sm" style={{ color: 'var(--t-muted)' }}>{i}</span>
                      </li>
                    ))}
                  </ul>
                )}
              </div>
            )}

            {error && (
              <p className="shrink-0 mt-body-sm" style={{ color: 'var(--severity-must)' }}>{error}</p>
            )}

            {/* The panels. 1-2 columns depending on count; each tile owns its own
                height so the grid divides the leftover space rather than growing. */}
            <div className="flex-1 min-h-0 grid gap-3" style={{
              gridTemplateColumns: summary.panels.length === 1 ? '1fr' : 'repeat(2, minmax(0, 1fr))',
              gridTemplateRows: summary.panels.length <= 2 ? '1fr' : 'repeat(2, minmax(0, 1fr))',
            }}>
              {summary.panels.map((p, i) => (
                <VegaPanel key={`${day}-${i}-${p.title}`} panel={p} data={data} />
              ))}
            </div>

            {/* The day's workstreams - the way into a worklog for any of them. */}
            {details.length > 0 && (
              <div className="shrink-0 flex items-center gap-2 flex-wrap">
                <span className="mt-body-sm shrink-0" style={{ color: 'var(--t-faint-2)' }}>
                  What you worked on:
                </span>
                {details.map(d => (
                  <button key={d.id} onClick={() => setSelected(d)}
                    className="rounded-full px-2.5 py-1 mt-body-sm transition-transform hover:-translate-y-px"
                    style={{
                      background: `color-mix(in srgb, ${d.hue} 12%, var(--t-card))`,
                      border: `1px solid color-mix(in srgb, ${d.hue} 34%, transparent)`,
                      color: 'var(--t-title)',
                    }}>
                    <span className="inline-block rounded-full mr-1.5 align-middle"
                      style={{ width: 6, height: 6, background: d.hue }} />
                    {d.title}
                  </button>
                ))}
              </div>
            )}
          </>
        )}
      </div>

      {/* The reused timeline detail panel, as a dialog. Generate worklog / approve /
          retarget / dismiss all come with it. */}
      <AnimatePresence>
        {selected && (
          <motion.div className="absolute inset-0 z-10 flex items-center justify-end p-4"
            initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }}
            transition={{ duration: 0.15 }}
            style={{ background: 'rgba(20,16,40,0.45)', backdropFilter: 'blur(2px)' }}
            onClick={() => setSelected(null)}>
            <motion.div onClick={e => e.stopPropagation()}
              initial={{ x: 24 }} animate={{ x: 0 }} exit={{ x: 24 }}
              transition={{ duration: 0.18 }}
              className="h-full rounded-2xl overflow-hidden shrink-0"
              style={{ width: 388, background: 'var(--t-panel)', border: '1px solid var(--t-card-border)' }}>
              <DayTaskDetailPanel
                detail={selected}
                onClose={() => setSelected(null)}
                onOpenSettings={onOpenSettings}
                onOpenTask={onOpenTask}
              />
            </motion.div>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  )
}

function Centered({ text }: { text: string }) {
  return (
    <div className="flex-1 min-h-0 flex items-center justify-center">
      <p className="mt-body-sm text-center" style={{ color: 'var(--t-faint-2)', maxWidth: '44ch' }}>
        {text}
      </p>
    </div>
  )
}
