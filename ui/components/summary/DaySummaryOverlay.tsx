//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The daily summary: one screen answering one question - did the day go the way
// you meant it to?
//
// TWO SCREENS, ONE LAYOUT. A day with a committed plan gets the segmented ring and
// a ledger of what became of each planned ticket. A day without one gets the same
// space given to what the day turned out to be about, sized by real measured time.
// Neither is a degraded version of the other, and the no-plan case is NOT nagged
// about the plan it did not make.
//
// THE PROSE IS STILL THE FEATURE. The headline and narrative get the type size and
// the room; the ring anchors them; everything else is quiet. The narrative renders
// `**emphasis**` (see Emphasis.tsx) so drift and breakthroughs can be said IN the
// sentence rather than stamped on a card as a category.
//
// This screen shipped once as prose squeezed above a grid of model-authored
// Vega-Lite charts, and it read as a monitoring dashboard - the one thing it must
// not be. There are no charts here now, and no chart library: every mark is a
// handful of SVG built for this screen.
//
// NUMBERS ARE NOT THE MODEL'S. The percentage, the counts and every duration are
// computed in Rust from the plan ledger (`day_evidence::adherence`), so the ring
// and the list beside it are two views of one array. The model contributes
// judgement and prose only.
//
// ONE SCREEN, NO PAGE SCROLL at the top level: min-h-0 all the way down, and the
// body scrolls internally when a long plan needs it.
//
// Clicking a workstream opens the SAME DayTaskDetailPanel the timeline uses, in a
// dialog - so generate/approve/retarget/dismiss all work here with no new worklog
// code (useWorklog is keyed by (day, taskId) in a module store, so a generate
// started here survives closing the dialog).

'use client'

import { useCallback, useEffect, useRef, useState } from 'react'
import { AnimatePresence, motion, useReducedMotion } from 'framer-motion'
import { load, invoke } from '@/lib/bridge'
import type { DaySummary, DaySummaryData, DayTask, DayTasksResponse } from '@/lib/api-types'
import { DayTaskDetailPanel, type DayTaskDetail } from '@/components/timeline/DayTaskDetailPanel'
import { taskHue } from '@/components/timeline/dayTaskKit'
import { hhmmToMin } from '@/components/timeline/dayTaskLayout'
import { formatDayLabel } from '@/components/timeline/types'
import { fmtDur } from '@/components/atoms'
import type { SettingsSection } from '@/components/timeline/settings/types'
import { AchievementRing } from './AchievementRing'
import { DayShape } from './DayShape'
import { Emphasis } from './Emphasis'
import { Insights } from './Insights'
import { PlanLedger } from './PlanLedger'
import { Workstreams } from './Workstreams'

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
      className="inline-flex items-center justify-center rounded-full transition-opacity disabled:opacity-25 hover:opacity-70"
      style={{ width: 26, height: 26, color: 'var(--t-muted)', border: '1px solid var(--t-hair)' }}>
      <span className="text-[13px] leading-none">{glyph}</span>
    </button>
  )
}

/** A hairline section break. Named so the three call sites stay identical - the
 *  rhythm of this screen is mostly the whitespace between its blocks. */
function Rule() {
  return <div className="shrink-0 my-6" style={{ height: 1, background: 'var(--t-hair)' }} />
}

export function DaySummaryOverlay({ day, isToday, onShiftDay, onClose, onOpenSettings, onOpenTask }: {
  day: string
  isToday: boolean
  onShiftDay: (delta: number) => void
  onClose: () => void
  onOpenSettings: (section?: SettingsSection) => void
  onOpenTask: (key: string, title?: string) => void
}) {
  const reduce = useReducedMotion()
  const [summary, setSummary] = useState<DaySummary | null>(null)
  const [data, setData] = useState<DaySummaryData | null>(null)
  const [tasks, setTasks] = useState<DayTask[]>([])
  const [loading, setLoading] = useState(true)
  const [generating, setGenerating] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [selected, setSelected] = useState<DayTaskDetail | null>(null)
  const [hovered, setHovered] = useState<number | null>(null)

  // Which days have already had their one silent refresh. A ref, not state: it must
  // not trigger a render, and it must survive the day flipping back and forth so a
  // user paging through the week cannot spend an LLM call per arrow press.
  const refreshed = useRef<Set<string>>(new Set())

  const generate = useCallback(async () => {
    setGenerating(true)
    setError(null)
    try {
      const s = await invoke<DaySummary>('generate_day_summary', { day })
      setSummary(s)
      // Re-read the live figures alongside: a regenerate can be minutes after the
      // last fold, and the deterministic half must bind to what is true now.
      await load<DaySummaryData>(API, 'get_day_summary_data', { day })
        .then(setData)
        .catch(() => {})
    } catch (e) {
      setError(e instanceof Error ? e.message : typeof e === 'string' ? e : 'Could not compose the summary')
    } finally {
      setGenerating(false)
    }
  }, [day])

  // Re-read everything on a day change. All three must come from the same day or
  // one day's ledger would render under another's prose.
  useEffect(() => {
    let cancelled = false
    setLoading(true)
    setSelected(null)
    setHovered(null)
    setError(null)
    Promise.allSettled([
      load<DaySummary | null>(API, 'get_day_summary', { day }),
      load<DaySummaryData>(API, 'get_day_summary_data', { day }),
      load<DayTasksResponse>('/api/day-tasks', 'get_day_tasks', { day }),
    ]).then(([s, d, t]) => {
      if (cancelled) return
      const existing = s.status === 'fulfilled' ? s.value : null
      const live = d.status === 'fulfilled' ? d.value : null
      setSummary(existing)
      setData(live)
      setTasks(t.status === 'fulfilled' ? (t.value?.tasks ?? []) : [])
      setLoading(false)

      // The day has moved on since this was written - recompose quietly, once.
      // Staleness is decided in Rust (`day_summaries::is_stale`); all this decides
      // is that it happens at most once per day per session, so reopening the
      // screen twice in a minute does not queue two calls.
      if (existing && live?.stale && !refreshed.current.has(day)) {
        refreshed.current.add(day)
        void generate()
      }
    })
    return () => { cancelled = true }
  }, [day, generate])

  const dayLabel = isToday ? 'Today' : formatDayLabel(day)
  const hasWork = tasks.length > 0
  const sc = data?.scalars
  // The plan branch comes from the LIVE scalars rather than the stored summary, so
  // a day planned after its summary was composed still gets the right screen.
  const planned = (sc?.planned ?? false) && (summary?.plan.length ?? 0) > 0

  return (
    <div className="absolute inset-0 z-50 flex flex-col rise" style={{ background: 'var(--win-bg)' }}>
      {/* Header — deliberately quiet. The day belongs to the body, not a title bar. */}
      <div className="shrink-0 flex items-center justify-between px-8 py-3">
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
              {generating ? 'Composing…' : summary ? 'Regenerate' : 'Compose summary'}
            </button>
          )}
          <button onClick={onClose} aria-label="Close"
            className="inline-flex items-center justify-center rounded-full hover:opacity-70"
            style={{ width: 28, height: 28, color: 'var(--t-muted)', border: '1px solid var(--t-hair)' }}>
            <span className="text-[16px] leading-none">×</span>
          </button>
        </div>
      </div>

      <div className="flex-1 min-h-0 flex flex-col px-8 pb-6">
        {loading ? (
          <Centered text="Reading your day…" />
        ) : !hasWork ? (
          <Centered text={`Nothing was tracked on ${dayLabel.toLowerCase()}, so there is no day to review.`} />
        ) : !summary ? (
          <Centered
            text={generating
              ? 'Reading the whole day and working out what is worth saying. This takes about a minute.'
              : 'Compose a summary to see how this day actually went.'}
          />
        ) : (
          // The one scroll on this screen. A ten-ticket plan plus a long day of
          // workstreams genuinely does not fit, and clipping it would be worse
          // than letting the body scroll under a fixed header.
          <div className="flex-1 min-h-0 overflow-y-auto -mx-2 px-2">
            {/* ── The hero ──────────────────────────────────────────────────── */}
            <div className="flex items-center gap-9 pt-2">
              {planned && summary.adherence.planned > 0 && (
                <AchievementRing
                  plan={summary.plan}
                  adherence={summary.adherence}
                  activeIndex={hovered}
                  onHover={setHovered}
                />
              )}

              <div className="flex-1 min-w-0">
                {summary.headline && (
                  <motion.h2
                    style={{
                      font: '800 25px var(--font-sans)',
                      letterSpacing: '-0.03em',
                      lineHeight: 1.15,
                      color: 'var(--t-title)',
                      maxWidth: '22ch',
                    }}
                    initial={reduce ? false : { opacity: 0, y: 6 }}
                    animate={{ opacity: 1, y: 0 }}
                    transition={{ duration: reduce ? 0 : 0.35, delay: reduce ? 0 : 0.1 }}
                  >
                    {summary.headline}
                  </motion.h2>
                )}

                {summary.narrative && (
                  <motion.p
                    className={summary.headline ? 'mt-3' : ''}
                    style={{
                      color: 'var(--t-muted)',
                      maxWidth: '56ch',
                      font: '450 16px/1.62 var(--font-sans)',
                      letterSpacing: '-0.008em',
                    }}
                    initial={reduce ? false : { opacity: 0, y: 6 }}
                    animate={{ opacity: 1, y: 0 }}
                    transition={{ duration: reduce ? 0 : 0.35, delay: reduce ? 0 : 0.16 }}
                  >
                    <Emphasis text={summary.narrative} />
                  </motion.p>
                )}

                {/* The unplanned tail, said plainly and WITHOUT apology - work that
                    was not on the plan is still work, and usually the interesting
                    half of the day. */}
                {planned && summary.adherence.unplanned_minutes > 0 && (
                  <p className="mt-body-sm mt-4" style={{ color: 'var(--t-faint-2)' }}>
                    {fmtDur(summary.adherence.unplanned_minutes * 60)} went to work that was not on the plan
                  </p>
                )}

                {summary.fallback && (
                  <p className="mt-body-sm mt-4" style={{ color: 'var(--t-faint-2)' }}>
                    The write-up could not be composed this time - what is below is measured, not written.
                  </p>
                )}
              </div>
            </div>

            {error && (
              <p className="mt-body-sm mt-4" style={{ color: 'var(--severity-must)' }}>{error}</p>
            )}

            {summary.insights.length > 0 && (
              <>
                <Rule />
                <Insights insights={summary.insights} delay={0.24} />
              </>
            )}

            {/* ── The plan, or what the day turned out to be ────────────────── */}
            {planned ? (
              <>
                <Rule />
                <p className="mt-label mb-1.5" style={{ color: 'var(--t-faint-2)' }}>
                  What you set out to do
                </p>
                <PlanLedger
                  plan={summary.plan}
                  activeIndex={hovered}
                  onHover={setHovered}
                  onOpenTask={onOpenTask}
                />
              </>
            ) : summary.themes.length > 0 ? (
              <>
                <Rule />
                <DayShape
                  themes={summary.themes}
                  tasks={tasks}
                  focusSeconds={sc?.focus_s ?? 0}
                  switchCount={sc?.switch_count ?? 0}
                />
              </>
            ) : null}

            {/* ── The way into a worklog for any of it ──────────────────────── */}
            <Rule />
            <Workstreams
              tasks={tasks}
              delay={0.3}
              onSelect={(t, i) => setSelected(detailOf(t, i, day))}
            />
          </div>
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
