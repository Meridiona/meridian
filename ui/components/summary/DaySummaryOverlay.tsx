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
import { Composing } from './Composing'
import { DayScore } from './DayScore'
import { DayShape } from './DayShape'
import { Insights } from './Insights'
import { WorkList } from './WorkList'

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

/** One labelled block of the summary.
 *
 *  `boxed` sets the content on its own quiet panel, and is for the two LISTS - a
 *  row of small type with a bar and a time on it needs an edge to sit against, or
 *  it floats in the middle of the page with nothing holding it. The hero and the
 *  insights are deliberately NOT boxed: they are prose, and prose in a box reads
 *  as a callout, which is the wrong emphasis on a screen whose whole point is
 *  that the writing comes first.
 *
 *  The label lives here rather than inside the list components so one thing owns
 *  the section's heading, its spacing, and its frame together. */
function Section({ label, boxed = false, children }: {
  label?: string
  boxed?: boolean
  children: React.ReactNode
}) {
  return (
    <section className="mt-7 first:mt-0">
      {label && (
        <p className="mt-label mb-2.5" style={{ color: 'var(--t-faint-2)' }}>{label}</p>
      )}
      {boxed ? (
        <div
          className="rounded-xl px-2 py-1.5"
          style={{ background: 'var(--t-box)', border: '1px solid var(--t-card-border)' }}
        >
          {children}
        </div>
      ) : (
        children
      )}
    </section>
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

  // Escape closes, as it does on every other overlay here (ModalShell's own
  // convention). It was missing while this screen was a full-bleed takeover, and
  // a card over a backdrop makes its absence obvious.
  useEffect(() => {
    function onKey(e: KeyboardEvent) { if (e.key === 'Escape') onClose() }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose])

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

  // Every tracked minute, sub-floor detours included: this is the "where did the day
  // go" figure, and rounding the short stretches out of it would make it disagree
  // with the timeline the reader just came from.
  const loggedMinutes = tasks.reduce((n, t) => n + t.minutes, 0)
  // Substantial work no planned ticket accounts for - counted here from the ledger's
  // own join so it cannot disagree with the rows below.
  const onPlan = new Set((summary?.plan ?? []).flatMap(v => v.day_task_ids))
  const bonusCount = tasks.filter(
    t => t.minutes >= (sc?.task_min_minutes ?? 30) && !onPlan.has(t.id),
  ).length

  return (
    // A card over the timeline, not a full-bleed takeover. The summary is a
    // composed artefact about one day - giving it edges, a shadow and air around
    // it is what makes it read as something made rather than as the app's
    // background with words on it. Same chrome as every other overlay here
    // (ModalShell / CleanupOverlay): dimmed blurred backdrop, one rounded panel.
    <div
      className="absolute inset-0 z-50 flex items-center justify-center p-5 sm:p-8 rise"
      style={{ background: 'rgba(20,16,40,0.5)', backdropFilter: 'blur(3px)' }}
      onClick={onClose}
    >
      <div
        // `relative` so the workstream detail dialog's `absolute inset-0` lands on
        // the card rather than on the backdrop behind it.
        className="relative w-full flex flex-col rounded-2xl overflow-hidden bg-panel"
        style={{
          maxWidth: 1000,
          // `maxHeight` with no fixed height, per ModalShell's note: a short day
          // sizes the card to its content instead of leaving a tall empty box,
          // while a long one still gets a definite height here - which is what
          // lets the body's own `flex-1 min-h-0 overflow-y-auto` clip and scroll.
          maxHeight: '100%',
          border: '1px solid var(--t-card-border)',
          boxShadow: 'var(--mt-modal-shadow)',
        }}
        onClick={e => e.stopPropagation()}
      >
        {/* Header — deliberately quiet, and pinned: the day nav and Regenerate stay
            reachable however far the body has scrolled. */}
        <div
          className="shrink-0 flex items-center justify-between px-7 py-3.5 border-b"
          style={{ borderColor: 'var(--t-hair)' }}
        >
          <div className="flex items-center gap-2.5 min-w-0">
            <NavBtn glyph="‹" label="Previous day" onClick={() => onShiftDay(-1)} />
            <p className="mt-label px-1 truncate" style={{ color: 'var(--t-muted)' }}>{dayLabel}</p>
            <NavBtn glyph="›" label="Next day" onClick={() => onShiftDay(1)} disabled={isToday} />
          </div>

          <div className="flex items-center gap-2.5 shrink-0">
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
              className="inline-flex items-center justify-center rounded-full bg-wrap hover:opacity-70"
              style={{ width: 30, height: 30, color: 'var(--t-muted)' }}>
              <span className="text-[17px] leading-none">×</span>
            </button>
          </div>
        </div>

        {/* `minHeight` so the empty and composing states get room to breathe now
            that the card sizes to its content - a spinner in a 90px-tall card
            reads as an error toast. */}
        <div className="flex-1 min-h-0 flex flex-col" style={{ minHeight: 340 }}>
        {loading ? (
          // Deliberately bare: this is a DB read that lands in milliseconds, and a
          // spinner for it would flash rather than inform.
          <Centered text="" />
        ) : !hasWork ? (
          <Centered text={`Nothing was tracked on ${dayLabel.toLowerCase()}, so there is no day to review.`} />
        ) : generating && !summary ? (
          <Composing planned={sc?.planned ?? false} />
        ) : !summary ? (
          <Centered
            title="How did this day go?"
            text="Compose a summary and Meridian will read the whole day back to you - what you got through, what took the time, and how it sat against your plan."
          />
        ) : (
          // The one scroll on this screen. A ten-ticket plan plus a long day of
          // workstreams genuinely does not fit, and clipping it would be worse
          // than letting the body scroll under a pinned header.
          <div className="flex-1 min-h-0 overflow-y-auto nice-scroll px-7 py-7">
            {/* ── The hero ──────────────────────────────────────────────────── */}
            {summary.headline && (
              <motion.h2
                style={{
                  font: '800 27px var(--font-sans)',
                  letterSpacing: '-0.032em',
                  lineHeight: 1.14,
                  color: 'var(--t-title)',
                  maxWidth: '26ch',
                }}
                initial={reduce ? false : { opacity: 0, y: 6 }}
                animate={{ opacity: 1, y: 0 }}
                transition={{ duration: reduce ? 0 : 0.35, delay: reduce ? 0 : 0.04 }}
              >
                {summary.headline}
              </motion.h2>
            )}

            {/* The numbers under the headline, then the sentence under them. The
                figures are what the eye lands on; the prose is what makes them
                mean something, and it reads better as a caption to them than as a
                paragraph they interrupt. */}
            <div className="mt-6">
              <DayScore
                plan={summary.plan}
                adherence={summary.adherence}
                planned={planned}
                loggedMinutes={loggedMinutes}
                bonusCount={bonusCount}
                workstreamCount={sc?.task_count ?? 0}
                focusSeconds={sc?.focus_s ?? 0}
              />
            </div>


            {summary.fallback && (
              <p className="mt-body-sm mt-4" style={{ color: 'var(--t-faint-2)' }}>
                The write-up could not be composed this time - what is below is measured, not written.
              </p>
            )}

            {error && (
              <p className="mt-body-sm mt-4" style={{ color: 'var(--severity-must)' }}>{error}</p>
            )}

            {summary.insights.length > 0 && (
              <Section>
                <Insights insights={summary.insights} delay={0.3} />
              </Section>
            )}

            {/* On a day with no plan there is no planned-vs-actual to draw, so the
                space goes to what the day turned out to be about instead. A day
                without a plan did not fail an exercise. */}
            {!planned && summary.themes.length > 0 && (
              <Section label="What the day was about">
                <DayShape
                  themes={summary.themes}
                  tasks={tasks}
                  focusSeconds={sc?.focus_s ?? 0}
                  switchCount={sc?.switch_count ?? 0}
                />
              </Section>
            )}

            {/* ── One list of the day, and the way into a worklog for any of it ── */}
            <Section label="What you worked on">
              <WorkList
                tasks={tasks}
                plan={planned ? summary.plan : []}
                planned={planned}
                allPlanDone={
                  planned &&
                  summary.adherence.planned > 0 &&
                  summary.adherence.done === summary.adherence.planned
                }
                delay={0.36}
                onSelect={(t, i) => setSelected(detailOf(t, i, day))}
                onOpenTask={onOpenTask}
              />
            </Section>
          </div>
        )}
      </div>

      {/* The reused timeline detail panel, as a dialog. Generate worklog / approve /
          retarget / dismiss all come with it. Inside the card, not over the whole
          screen: it is a detail OF this summary, and stacking a second full-screen
          scrim over the first would bury the card it belongs to. */}
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
    </div>
  )
}

/** An empty state. `title` lifts it from a notice into an invitation, which is what
 *  the pre-compose screen needs and the nothing-was-tracked one does not. */
function Centered({ title, text }: { title?: string; text: string }) {
  return (
    <div className="flex-1 min-h-0 flex flex-col items-center justify-center gap-3">
      {title && (
        <p style={{
          font: '800 21px var(--font-sans)',
          letterSpacing: '-0.025em',
          color: 'var(--t-title)',
        }}>
          {title}
        </p>
      )}
      <p className="mt-body text-center" style={{ color: 'var(--t-faint-2)', maxWidth: '46ch', lineHeight: 1.6 }}>
        {text}
      </p>
    </div>
  )
}
