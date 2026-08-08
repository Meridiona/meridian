//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The daily summary: one screen answering one question - did the day go the way
// you meant it to?
//
// ONE LAYOUT, TWO CASES. A day with a committed plan gets the segmented ring; a day
// without one is the same screen minus the ring - title, the three cards, the
// checklist. The no-plan case is NOT nagged about the plan it did not make, and gets
// no themed stand-in for one.
//
// THE THREE CARDS ARE THE PROSE. The headline sits over a deterministic stat block,
// and the three insight cards carry everything the model has to say about the day.
//
// This screen shipped once as prose squeezed above a grid of model-authored
// Vega-Lite charts, and it read as a monitoring dashboard - the one thing it must
// not be. There are no charts here now, and no chart library: every mark is a
// handful of SVG built for this screen.
//
// NUMBERS ARE NOT THE MODEL'S - AND NEITHER IS THE PLAN LEDGER. The percentage, the
// counts, every duration, and which tickets are ticked are resolved in Rust from the
// worklog matches (`day_evidence::adherence::resolve_deterministic`), with no LLM.
// The model writes only the headline and the cards, so the ring and checklist hold
// even when the prose call fails.
//
// ONE SCREEN, NO PAGE SCROLL at the top level: min-h-0 all the way down, and the
// body scrolls internally when a long plan needs it.
//
// THE BOTTOM HALF IS TWO COLUMNS: the plan (and what came up that was not on it) on
// the left, the standup on the right. They are side by side because they are the two
// OUTPUTS of the day - what happened, and what you will say about it - and reading
// one immediately after the other is what makes the second believable. This is the
// arrangement the first-run walkthrough teaches (`TutorialSummaryCard`), and the two
// are kept in step deliberately: a tour that teaches a screen the product does not
// have has taught nothing.
//
// TWO VIEWS, one card. This is the DAY; `SummaryTaskView` is one strand of it,
// opened by clicking a row. It replaced a right-anchored `DayTaskDetailPanel`
// drawer - see that file for why the document leads here and the evidence does not.
// `useWorklog` is keyed by (day, taskId) in a module store, so a generate started
// here survives going back to the day and returning.

'use client'

import { useCallback, useEffect, useRef, useState } from 'react'
import { motion, useReducedMotion } from 'framer-motion'
import { load, invoke } from '@/lib/bridge'
import type { DaySummary, DaySummaryData, DayDraftState, DayTask, DayTasksResponse } from '@/lib/api-types'
import { type DayTaskDetail } from '@/components/timeline/DayTaskDetailPanel'
import { taskHue } from '@/components/timeline/dayTaskKit'
import { hhmmToMin } from '@/components/timeline/dayTaskLayout'
import { formatDayLabel } from '@/components/timeline/types'
import type { SettingsSection } from '@/components/timeline/settings/types'
import type { RuntimeSettings } from '@/lib/settings'
import { Composing } from './Composing'
import { DayScore } from './DayScore'
import { Insights } from './Insights'
import { Standup } from './Standup'
import { SummaryTaskView } from './SummaryTaskView'
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

/** "DAILY SUMMARY · WED 22 JUL" - the eyebrow over the headline.
 *
 *  Parsed into a local `Date` from the parts rather than `new Date(day)`, which
 *  reads a bare "YYYY-MM-DD" as UTC midnight and therefore prints the PREVIOUS day
 *  for anyone west of Greenwich. */
function eyebrowDate(day: string): string {
  const [y, m, d] = day.split('-').map(Number)
  if (!y || !m || !d) return ''
  return new Date(y, m - 1, d)
    .toLocaleDateString(undefined, { weekday: 'short', day: 'numeric', month: 'short' })
    .toUpperCase()
}

/** The one deterministic sentence on the screen, under the headline.
 *
 *  Every figure in it is measured (`day_evidence::adherence`), which is why it can
 *  sit above the model's cards without the fallback path having to hide it. It says
 *  what the numbers beside it mean and nothing more - the reading of the day is the
 *  insight cards' job, not this line's. */
function heroLine(planned: boolean, done: number, total: number, bonusCount: number): string {
  if (!planned) {
    return bonusCount > 0
      ? 'No plan was set for this day, so here is what it actually went on.'
      : 'No plan was set for this day - here is what it actually went on.'
  }
  const wrapped = `You wrapped ${done} of ${total} planned ${total === 1 ? 'task' : 'tasks'}.`
  if (bonusCount === 0) return wrapped
  const extra =
    bonusCount === 1
      ? 'One more thing came up that was not on the plan - it is below, with a draft ready.'
      : `${bonusCount} more things came up that were not on the plan - they are below, with drafts ready.`
  return `${wrapped} ${extra}`
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
  /** Worklog state per task id. `undefined` until the read lands (or if it failed) -
   *  which renders the rows badgeless rather than guessing at a state. */
  const [drafts, setDrafts] = useState<Map<string, DayDraftState> | undefined>(undefined)
  const [loading, setLoading] = useState(true)
  const [generating, setGenerating] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [selected, setSelected] = useState<DayTaskDetail | null>(null)
  // The end-of-day time the user chose, if any — drives the pre-compose copy that
  // tells them when this builds itself. `null` = no time set (never scheduled).
  const [autoTime, setAutoTime] = useState<string | null>(null)

  useEffect(() => {
    load<RuntimeSettings>('/api/settings', 'get_settings')
      .then(s => setAutoTime(s.worklog_auto_generate_time))
      .catch(() => {})
  }, [])

  // Which days have already had their one silent refresh. A ref, not state: it must
  // not trigger a render, and it must survive the day flipping back and forth so a
  // user paging through the week cannot spend an LLM call per arrow press.
  const refreshed = useRef<Set<string>>(new Set())

  // The day currently on screen, mirrored into a ref so an in-flight compose can
  // tell whether the user navigated away before it resolved (and must not clobber
  // the new day's state - see `generate`/`generateNow`).
  const dayRef = useRef(day)
  useEffect(() => { dayRef.current = day }, [day])

  // A re-entry guard shared by both compose paths: a fast double-click (or a
  // Regenerate + Generate-now overlap) must not fire two concurrent LLM calls. A
  // ref rather than the `generating` state so the check sees the latest value
  // synchronously, before React has re-rendered.
  const composing = useRef(false)

  // Escape closes, as it does on every other overlay here (ModalShell's own
  // convention). When the nested ticket-detail dialog is open, Escape backs out of
  // THAT first (it has no Escape handling of its own) rather than closing the whole
  // summary from under it.
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key !== 'Escape') return
      if (selected) { setSelected(null); return }
      onClose()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose, selected])

  // Recompose the prose only (Regenerate on an existing summary, and the quiet
  // staleness refresh). The deterministic ledger binds to whatever worklogs are
  // already drafted — it does not draft new ones.
  const generate = useCallback(async () => {
    if (composing.current) return
    composing.current = true
    setGenerating(true)
    setError(null)
    try {
      const s = await invoke<DaySummary>('generate_day_summary', { day })
      // The user may have paged to another day while this ran - a late response
      // must not overwrite the day now on screen with the old day's content.
      if (dayRef.current !== day) return
      setSummary(s)
      // Re-read the live figures alongside: a regenerate can be minutes after the
      // last fold, and the deterministic half must bind to what is true now.
      const d = await load<DaySummaryData>(API, 'get_day_summary_data', { day }).catch(() => null)
      if (dayRef.current !== day) return
      if (d) setData(d)
    } catch (e) {
      if (dayRef.current !== day) return
      setError(e instanceof Error ? e.message : typeof e === 'string' ? e : 'Could not compose the summary')
    } finally {
      composing.current = false
      if (dayRef.current === day) setGenerating(false)
    }
  }, [day])

  // "Generate now" — the same work the end-of-day pass does, on demand: draft the
  // day's worklogs FIRST, then compose over the fresh matches. Slow (a worklog is
  // one LLM call per qualifying task), so the Composing state covers the wait.
  const generateNow = useCallback(async () => {
    if (composing.current) return
    composing.current = true
    setGenerating(true)
    setError(null)
    try {
      const s = await invoke<DaySummary>('generate_day_summary_now', { day })
      if (dayRef.current !== day) return
      setSummary(s)
      const d = await load<DaySummaryData>(API, 'get_day_summary_data', { day }).catch(() => null)
      if (dayRef.current !== day) return
      if (d) setData(d)
    } catch (e) {
      if (dayRef.current !== day) return
      setError(e instanceof Error ? e.message : typeof e === 'string' ? e : 'Could not generate the summary')
    } finally {
      composing.current = false
      if (dayRef.current === day) setGenerating(false)
    }
  }, [day])

  // Re-read everything on a day change. All three must come from the same day or
  // one day's ledger would render under another's prose.
  useEffect(() => {
    let cancelled = false
    setLoading(true)
    setSelected(null)
    setError(null)
    Promise.allSettled([
      load<DaySummary | null>(API, 'get_day_summary', { day }),
      load<DaySummaryData>(API, 'get_day_summary_data', { day }),
      load<DayTasksResponse>('/api/day-tasks', 'get_day_tasks', { day }),
      // Which of the day's rows have a worklog waiting on the user. A local DB read,
      // fetched alongside the rest rather than after, so the badges are on the rows in
      // the first paint - arriving late would move the list under someone reading it.
      // Failure is not fatal: no map means no badges, and the rows are still correct.
      load<DayDraftState[]>(API, 'get_day_draft_states', { day }),
    ]).then(([s, d, t, w]) => {
      if (cancelled) return
      const existing = s.status === 'fulfilled' ? s.value : null
      const live = d.status === 'fulfilled' ? d.value : null
      setSummary(existing)
      setData(live)
      setTasks(t.status === 'fulfilled' ? (t.value?.tasks ?? []) : [])
      setDrafts(
        w.status === 'fulfilled' && Array.isArray(w.value)
          ? new Map(w.value.map((r) => [r.task_id, r]))
          : undefined,
      )
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
  // The plan branch comes SOLELY from the LIVE scalars, not the stored summary, so
  // a day planned after its summary was composed still gets the plan screen rather
  // than being routed to the no-plan layout. The ledger the ring/checklist read
  // (`summary.plan`) may lag until the next compose; a plan screen with a not-yet-
  // recomposed ledger is the right screen, whereas the no-plan screen is the wrong
  // one. (ANDing in `summary.plan.length` here was the bug: it flipped a genuinely
  // planned day to the no-plan layout whenever the plan post-dated the summary.)
  const planned = sc?.planned ?? false

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
        // `relative` so anything absolutely positioned inside the card lands on IT
        // rather than on the backdrop behind it, which is the nearest positioned
        // ancestor otherwise.
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
            {/* Only once a summary exists: pre-compose, the CTA lives in the body
                where the scheduling copy explains what "Generate now" will do. */}
            {hasWork && summary && (
              <button onClick={generate} disabled={generating}
                className="rounded-full px-3.5 py-1.5 mt-label transition-opacity disabled:opacity-50 hover:opacity-85"
                style={{
                  background: 'transparent',
                  color: 'var(--t-muted)',
                  border: '1px solid var(--t-hair)',
                }}>
                {generating ? 'Composing…' : 'Regenerate'}
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
          <PreCompose autoTime={autoTime} onGenerate={generateNow} />
        ) : (
          // The one scroll on this screen. A ten-ticket plan plus a long day of
          // workstreams genuinely does not fit, and clipping it would be worse
          // than letting the body scroll under a pinned header.
          //
          // Keyed on which view is in the card, so opening a row and coming back
          // fades rather than cuts. The two views share nothing visually - the whole
          // day, then one strand of it - so a hard swap reads as a different window
          // appearing in the same frame.
          <div key={selected ? 'task' : 'home'}
            className="flex-1 min-h-0 overflow-y-auto nice-scroll px-7 py-7 rise">
            {selected ? (
              <SummaryTaskView
                detail={selected}
                onBack={() => setSelected(null)}
                onOpenSettings={onOpenSettings}
                onOpenTask={onOpenTask}
              />
            ) : (
              <>
            {/* ── Hero: the day in a sentence, and the numbers behind it ─────── */}
            <div className="flex items-start gap-8">
              <div className="min-w-0 flex-1">
                <p className="mt-label" style={{ color: 'var(--accent)' }}>
                  DAILY SUMMARY · {eyebrowDate(day)}
                </p>
                {summary.headline && (
                  <motion.h2
                    className="mt-2"
                    style={{
                      font: '800 25px var(--font-sans)',
                      letterSpacing: '-0.03em',
                      lineHeight: 1.15,
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
                <p className="mt-body mt-3" style={{ color: 'var(--t-muted)', lineHeight: 1.55, maxWidth: '46ch' }}>
                  {heroLine(planned, summary.adherence.done, summary.adherence.planned, bonusCount)}
                </p>
              </div>

              <div className="shrink-0">
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
            </div>

            {summary.fallback && (
              <p className="mt-body-sm mt-4" style={{ color: 'var(--t-faint-2)' }}>
                The write-up could not be composed this time - what is below is measured, not written.
              </p>
            )}

            {error && (
              <p className="mt-body-sm mt-4" style={{ color: 'var(--severity-must)' }}>{error}</p>
            )}

            {/* ── The two or three things worth remembering ──────────────────── */}
            {summary.insights.length > 0 && (
              <div className="mt-6">
                <Insights insights={summary.insights} delay={0.3} />
              </div>
            )}

            {/* ── The plan, ticked off, and the standup it wrote ───────────────
                Side by side because they are the two OUTPUTS of the day - what
                happened, and what you will say about it tomorrow - and reading one
                straight after the other is what makes the second believable.
                The standup renders nothing when the model gave no lines (the
                fallback path, or a pre-078 row), so the grid collapses to the list
                alone rather than leaving an empty captioned box beside it. */}
            <div
              className="grid gap-3 mt-6"
              style={{ gridTemplateColumns: summary.standup.length > 0 ? '1.15fr 1fr' : '1fr' }}
            >
              <WorkList
                tasks={tasks}
                plan={planned ? summary.plan : []}
                planned={planned}
                drafts={drafts}
                delay={0.36}
                onSelect={(t, i) => setSelected(detailOf(t, i, day))}
                onOpenTask={onOpenTask}
              />
              <Standup lines={summary.standup} />
            </div>
              </>
            )}
          </div>
        )}
      </div>

      </div>
    </div>
  )
}

/** "18:00" → "6:00 PM". Falls back to the raw string on anything unparseable. */
function fmtClock(hhmm: string): string {
  const [h, m] = hhmm.split(':').map(Number)
  if (Number.isNaN(h) || Number.isNaN(m)) return hhmm
  const ampm = h < 12 ? 'AM' : 'PM'
  const h12 = h % 12 === 0 ? 12 : h % 12
  return `${h12}:${String(m).padStart(2, '0')} ${ampm}`
}

/** The pre-compose screen: what happens on its own, and the way to do it now.
 *
 *  When an end-of-day time is set, this is not an empty state but a promise - at
 *  that time Meridian drafts the worklogs, composes this summary, and notifies the
 *  user to review it. "Generate now" is the same work, on demand, for when they
 *  don't want to wait. With no time set, it is an invitation to do it now (and a
 *  pointer to schedule it). */
function PreCompose({ autoTime, onGenerate }: {
  autoTime: string | null
  onGenerate: () => void
}) {
  return (
    <div className="flex-1 min-h-0 flex flex-col items-center justify-center gap-4 px-8 text-center">
      <p style={{
        font: '800 21px var(--font-sans)',
        letterSpacing: '-0.025em',
        color: 'var(--t-title)',
      }}>
        How did this day go?
      </p>
      <p className="mt-body" style={{ color: 'var(--t-faint-2)', maxWidth: '50ch', lineHeight: 1.6 }}>
        {autoTime
          ? `At ${fmtClock(autoTime)} Meridian will draft your worklogs and put this summary together, then notify you to review them. You can do it now instead.`
          : 'Meridian will draft your worklogs and read the whole day back to you - what you got through, what took the time, and how it sat against your plan. Set an end-of-day time in Settings to have this build itself, or do it now.'}
      </p>
      <button onClick={onGenerate}
        className="rounded-full px-5 py-2 mt-label transition-opacity hover:opacity-85"
        style={{ background: 'var(--t-title)', color: 'var(--t-panel)', fontWeight: 700 }}>
        Generate now
      </button>
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
