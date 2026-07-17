//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The daily summary: one screen, composed by the model, about how a day went.
//
// THE PROSE IS THE FEATURE. This screen exists to tell someone how their day went
// in words they would actually want to read, and to make them feel good about it.
// The narrative gets the room and the type size; the stat row gives it three honest
// anchors; charts are OPTIONAL (0-2, the model decides) and sit underneath.
//
// It shipped once the other way round — four charts in a grid with the prose
// squeezed into a strip above them — and it read as a monitoring dashboard, which is
// the one thing it must not be. If something has to give here, it is the charts.
//
// ONE SCREEN, NO PAGE SCROLL: min-h-0 all the way down, and the panel count is
// capped server-side (MAX_PANELS = 2).
//
// Nothing here decides what to show. The narrative, the insights, which charts and
// what form each takes all come from the model; this lays them out. The specs carry
// FORM only, so the real rows are fetched separately (get_day_summary_data) and
// injected by VegaPanel at render.
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

/** Seconds as the home page writes them ("3h 47m"), so the two agree on sight. */
function fmtDur(s: number): string {
  const h = Math.floor(s / 3600)
  const m = Math.round((s % 3600) / 60)
  if (h === 0) return `${m}m`
  return m === 0 ? `${h}h` : `${h}h ${m}m`
}

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
      className="inline-flex items-center justify-center rounded-full transition-opacity disabled:opacity-25 hover:opacity-70"
      style={{ width: 26, height: 26, color: 'var(--t-muted)', border: '1px solid var(--t-hair)' }}>
      <span className="text-[13px] leading-none">{glyph}</span>
    </button>
  )
}

/** One headline number. Big value, quiet label — the value is the thing. */
function Stat({ value, label }: { value: string; label: string }) {
  return (
    <div className="shrink-0">
      <p className="leading-none" style={{
        color: 'var(--t-title)', fontSize: 30, fontWeight: 600, letterSpacing: '-0.02em',
      }}>
        {value}
      </p>
      <p className="mt-label mt-1.5" style={{ color: 'var(--t-faint-2)' }}>{label}</p>
    </div>
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
  const [data, setData] = useState<DaySummaryData | null>(null)
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
      setData(d.status === 'fulfilled' ? d.value : null)
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
  const sc = data?.scalars
  const panels = summary?.panels ?? []

  // The workstreams, in the timeline's own order and colours, so a task looks the
  // same here as it does there.
  const details = useMemo(() => tasks.map((t, i) => detailOf(t, i, day)), [tasks, day])

  return (
    <div className="absolute inset-0 z-50 flex flex-col rise" style={{ background: 'var(--win-bg)' }}>
      {/* Header — deliberately quiet. The day belongs to the body, not a title bar. */}
      <div className="shrink-0 flex items-center justify-between px-6 py-3">
        <div className="flex items-center gap-2.5 min-w-0">
          <NavBtn glyph="‹" label="Previous day" onClick={() => onShiftDay(-1)} />
          <p className="mt-label px-1 truncate" style={{ color: 'var(--t-muted)' }}>{dayLabel}</p>
          <NavBtn glyph="›" label="Next day" onClick={() => onShiftDay(1)} disabled={isToday} />
        </div>

        <div className="flex items-center gap-2.5 shrink-0">
          {summary && !summary.fallback && (
            <span className="mt-body-sm" style={{ color: 'var(--t-faint-2)' }}>
              {summary.model || summary.provider}
            </span>
          )}
          {hasWork && (
            <button onClick={generate} disabled={generating}
              className="rounded-full px-3.5 py-1.5 mt-label transition-opacity disabled:opacity-50 hover:opacity-85"
              style={{
                background: summary ? 'transparent' : 'var(--accent)',
                color: summary ? 'var(--t-muted)' : '#fff',
                border: summary ? '1px solid var(--t-hair)' : '1px solid transparent',
              }}>
              {generating ? 'Composing…' : summary ? 'Regenerate' : 'Generate summary'}
            </button>
          )}
          <button onClick={onClose} aria-label="Close"
            className="inline-flex items-center justify-center rounded-full hover:opacity-70"
            style={{ width: 28, height: 28, color: 'var(--t-muted)', border: '1px solid var(--t-hair)' }}>
            <span className="text-[16px] leading-none">×</span>
          </button>
        </div>
      </div>

      {/* Body. min-h-0 all the way down is what keeps this inside the viewport
          instead of growing a page scrollbar. */}
      <div className="flex-1 min-h-0 flex flex-col px-6 pb-5">
        {loading ? (
          <Centered text="Reading your day…" />
        ) : !hasWork ? (
          <Centered text={`Nothing was tracked on ${dayLabel.toLowerCase()}, so there is no day to review.`} />
        ) : !summary ? (
          <Centered
            text={generating
              ? 'Reading the whole day and deciding what is worth saying. This takes about a minute.'
              : 'Compose a summary to see how this day actually went.'}
          />
        ) : (
          <>
            {/* The headline row. `task_count` excludes anything under half an hour
                — see TASK_MIN_MINUTES — because a count that includes every glance
                is a number the reader sees through instantly. */}
            {sc && (
              <div className="shrink-0 flex items-start gap-10 pt-1 pb-6">
                <Stat
                  value={String(sc.task_count)}
                  label={sc.task_count === 1 ? 'thing you moved forward' : 'things you moved forward'}
                />
                <div className="shrink-0 self-stretch" style={{ width: 1, background: 'var(--t-hair)' }} />
                <Stat value={fmtDur(sc.focus_s)} label="focus" />
                {sc.coding_s > 0 && <Stat value={fmtDur(sc.coding_s)} label="coding" />}
              </div>
            )}

            {/* The narrative. The biggest text on the screen, and the whole point of
                it. `panels.length === 0` is the COMMON, correct case — when the model
                chose no chart, the prose takes the room rather than the layout
                leaving a hole where a chart was supposed to be. */}
            <div className={panels.length === 0 ? 'flex-1 min-h-0 flex flex-col justify-center' : 'shrink-0'}>
              {summary.narrative && (
                <p style={{
                  color: 'var(--t-title)',
                  maxWidth: '58ch',
                  fontSize: panels.length === 0 ? 25 : 19,
                  lineHeight: 1.55,
                  letterSpacing: '-0.011em',
                  fontWeight: 400,
                }}>
                  {summary.narrative}
                </p>
              )}

              {summary.insights.length > 0 && (
                <ul className="flex flex-col gap-2 mt-6" style={{ maxWidth: '58ch' }}>
                  {summary.insights.map((i, n) => (
                    <li key={n} className="flex gap-3 items-baseline">
                      <span className="shrink-0 rounded-full" style={{
                        width: 4, height: 4, background: 'var(--accent)', transform: 'translateY(-2px)',
                      }} />
                      <span className="mt-body" style={{ color: 'var(--t-muted)' }}>{i}</span>
                    </li>
                  ))}
                </ul>
              )}

              {summary.fallback && (
                <p className="mt-body-sm mt-4" style={{ color: 'var(--t-faint-2)' }}>
                  A plain view of the day - the summary could not be composed this time.
                </p>
              )}
            </div>

            {error && (
              <p className="shrink-0 mt-body-sm mt-3" style={{ color: 'var(--severity-must)' }}>{error}</p>
            )}

            {/* The panels, if the model wanted any. Rendered ONLY when present — no
                empty frames, no placeholder. */}
            {panels.length > 0 && data && (
              <div className="flex-1 min-h-0 grid gap-4 mt-6" style={{
                gridTemplateColumns: panels.length === 1 ? '1fr' : 'repeat(2, minmax(0, 1fr))',
              }}>
                {panels.map((p, i) => (
                  <VegaPanel key={`${day}-${i}-${p.title}`} panel={p} data={data.datasets} />
                ))}
              </div>
            )}

            {/* The day's workstreams — the way into a worklog for any of them. */}
            {details.length > 0 && (
              <div className="shrink-0 flex items-center gap-2 flex-wrap pt-5">
                {details.map(d => (
                  <button key={d.id} onClick={() => setSelected(d)}
                    title={`${d.title} - ${d.minutes} min`}
                    className="rounded-full px-3 py-1.5 mt-body-sm transition-transform hover:-translate-y-px"
                    style={{
                      background: `color-mix(in srgb, ${d.hue} 10%, var(--t-card))`,
                      border: `1px solid color-mix(in srgb, ${d.hue} 28%, transparent)`,
                      color: 'var(--t-title)',
                      maxWidth: 260,
                    }}>
                    <span className="inline-block rounded-full mr-2 align-middle"
                      style={{ width: 6, height: 6, background: d.hue }} />
                    <span className="align-middle truncate inline-block" style={{ maxWidth: 210 }}>
                      {d.title}
                    </span>
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
      <p className="mt-body text-center" style={{ color: 'var(--t-faint-2)', maxWidth: '44ch' }}>
        {text}
      </p>
    </div>
  )
}
