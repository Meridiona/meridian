//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// The day-task timeline — the left column of the one-pager. It draws Meridian's
// own inferred day-level tasks (workstreams) as vertical blocks placed at their
// real approximate times. Each task is drawn from its SEGMENTS: the stretches it
// was actually worked. A task worked in two sittings (08:01-08:59, then
// 09:31-09:39) shows as two solid bars in ONE lane, the same colour, with the gap
// between them reading as a break in the same workstream. Data is `get_day_tasks`,
// folded hour by hour by the worklog pipeline.
//
// Self-contained (fetches + polls its own data, like OverviewPanel's plan) so the
// shell only passes the day. Clicking a task opens a detail card with its time
// breakdown and running summary; the whole point is the day reads as a handful of
// workstreams placed where the time actually went, not 24 rows.

import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { fmtDur } from '@/components/atoms'
import { load } from '@/lib/bridge'
import type { DayTask, DayTasksResponse } from '@/lib/api-types'
import {
  layoutDayTasks,
  taskWindow,
  taskRangeLabel,
  hourClock,
  type LaidOutTask,
} from './dayTaskLayout'
import type { DayTaskDetail } from './DayTaskDetailPanel'
import { taskHue, Bullets } from './dayTaskKit'

// The vertical scale (px per minute) is DYNAMIC — the timeline fits itself to the
// pane. A sparse day (one short task, the first hour after a fresh fold) would
// collapse to a few unreadable pixels at a fixed scale, so instead we stretch the
// content window to fill the available height. Bounds keep it honest: a dense full
// day floors at MIN_PX_PER_MIN and scrolls; a near-empty day caps at MAX_PX_PER_MIN
// so a single task fills the pane without becoming absurd.
const MIN_PX_PER_MIN = 0.55 // ~33 px/hr — a long day stays compact, scrolls if needed
const MAX_PX_PER_MIN = 9 // ~540 px/hr — a lone short task still reads big
const FALLBACK_PANE_PX = 560 // used before the pane is measured (first paint / SSR)
const GUTTER = 58 // px reserved on the left for hour labels
// A task card never draws thinner than this, so even a few-minute workstream stays
// visible, labelled, and tappable (a small honest over-draw for very short work).
const MIN_SEG_PX = 22
// The left colour rail floats this many px in from the card's top and bottom
// edges so it never touches the rounded corners (a solid bar flush to the
// boundary reads as a broken border, not a rail).
const RAIL_INSET = 10
// A hair of extra vertical room folded into lane collision, so side-by-side cards
// (and stacked ones) read as clearly separate rather than flush against each other.
const CARD_SEP_PX = 6
// Height thresholds that decide how much of a card's content is shown. Below
// COMPACT_MAX_PX a card is a single-line pill (dot + title); above it the full
// header appears, then the meta row, then as many summary-preview lines as fit —
// so a tall card is filled with what was actually done rather than empty space.
const COMPACT_MAX_PX = 46
const META_MIN_PX = 62
const SUMMARY_MIN_PX = 104
const SUMMARY_LINE_PX = 17 // approx line height of a preview line

export function DayTaskColumn({ day, isToday, selectedId, onSelect, tasks }: {
  day: string
  isToday: boolean
  // Selection is owned by the shell so the clicked task's detail can render in
  // the right panel ("Today at a glance") — this column only highlights the
  // selected card and dims the rest.
  selectedId: string | null
  onSelect: (detail: DayTaskDetail | null) => void
  // Injected task set (the dev-only LLM Lab feeds each model's simulated day
  // straight into this real timeline). When set, the column renders exactly
  // these tasks and never fetches or polls — the caller owns the data.
  tasks?: DayTask[]
}) {
  const [resp, setResp] = useState<DayTasksResponse | null>(null)
  const [loaded, setLoaded] = useState(false)
  const selected = selectedId

  // Available vertical space for the timeline, measured live so the scale can fit
  // the content to the pane (see MIN/MAX_PX_PER_MIN). scrollRef is the scrolling
  // viewport; headerRef is the title block we subtract from it.
  const scrollRef = useRef<HTMLDivElement>(null)
  const headerRef = useRef<HTMLDivElement>(null)
  const [paneH, setPaneH] = useState(FALLBACK_PANE_PX)
  useLayoutEffect(() => {
    const el = scrollRef.current
    if (!el) return
    const measure = () => {
      const avail = el.clientHeight - (headerRef.current?.offsetHeight ?? 0) - 40 // pb-8 + breathing
      setPaneH(Math.max(200, avail))
    }
    measure()
    const ro = new ResizeObserver(measure)
    ro.observe(el)
    if (headerRef.current) ro.observe(headerRef.current)
    return () => ro.disconnect()
  }, [loaded])

  useEffect(() => {
    // Injected data: no fetch, no poll — render what the caller provided.
    if (tasks) { setResp({ day, tasks }); setLoaded(true); return }
    let alive = true
    const fetchTasks = () =>
      load<DayTasksResponse>('/api/day-tasks', 'get_day_tasks', { day })
        .then(r => { if (alive) { setResp(r); setLoaded(true) } })
        .catch(() => { if (alive) setLoaded(true) })
    fetchTasks()
    // Only the live day keeps changing; a past day is settled.
    const id = isToday ? setInterval(fetchTasks, 30_000) : undefined
    return () => { alive = false; if (id) clearInterval(id) }
  }, [day, isToday, tasks])

  // Two-pass layout: the window (and thus the pixel scale) depends only on the
  // tasks' real footprints, so lay out once lane-agnostically to size the column,
  // then lay out again passing the minutes the pixel floor represents at that scale
  // — that is what keeps near-touching short cards in separate side-by-side lanes.
  const laidBase = useMemo(() => layoutDayTasks(resp?.tasks ?? []), [resp])
  const win = useMemo(() => taskWindow(laidBase), [laidBase])
  const windowMin = Math.max(1, win.hi - win.lo)
  const pxPerMin = Math.max(
    MIN_PX_PER_MIN,
    Math.min(MAX_PX_PER_MIN, paneH / windowMin),
  )
  const colHeight = windowMin * pxPerMin
  // A card visually occupies at least MIN_SEG_PX; add a hair of separation so
  // adjacent lanes read as distinct. Convert that pixel floor back into minutes.
  const minGapMin = (MIN_SEG_PX + CARD_SEP_PX) / pxPerMin
  const laid = useMemo(
    () => layoutDayTasks(resp?.tasks ?? [], minGapMin),
    [resp, minGapMin],
  )
  const firstHour = Math.floor(win.lo / 60)
  const lastHour = Math.ceil(win.hi / 60)
  const hourLines = useMemo(
    () => Array.from({ length: lastHour - firstHour + 1 }, (_, i) => firstHour + i),
    [firstHour, lastHour],
  )

  const dayLabel = isToday ? 'Today' : day
  const taskCount = laid.length

  return (
    // A click anywhere in the column that isn't on a card clears the selection
    // (returns the right panel to the day's glance) — cards stopPropagation so
    // clicking one still just selects/toggles it.
    <div className="relative h-full" onClick={() => { if (selected) onSelect(null) }}>
      <div ref={scrollRef} className="h-full overflow-y-auto nice-scroll">
      <div ref={headerRef} className="px-6 pt-6 pb-2 flex items-baseline justify-between">
        <div>
          <p className="mt-label" style={{ color: 'var(--t-faint)' }}>{dayLabel}</p>
          <p className="mt-greeting text-title mt-1" style={{ fontSize: 20 }}>
            {taskCount > 0 ? `${taskCount} task${taskCount === 1 ? '' : 's'} today` : 'Your day, in tasks'}
          </p>
        </div>
        {taskCount > 0 && (
          <p className="mt-body-sm" style={{ color: 'var(--t-faint)' }}>
            {fmtDur(laid.reduce((a, l) => a + l.task.minutes, 0) * 60)} tracked
          </p>
        )}
      </div>

      {!loaded ? null : taskCount === 0 ? (
        <EmptyState isToday={isToday} />
      ) : (
        <div className="px-6 pb-8">
          <div className="relative" style={{ height: colHeight }}>
            {/* Hour gridlines + labels */}
            {hourLines.map(h => {
              const top = (h * 60 - win.lo) * pxPerMin
              if (top < -1 || top > colHeight + 1) return null
              return (
                <div key={h} className="absolute left-0 right-0 flex items-start"
                  style={{ top, height: 0 }}>
                  <span className="mt-mono-sm shrink-0 -translate-y-1/2"
                    style={{ width: GUTTER - 12, fontSize: 10.5, color: 'var(--t-faint)', textAlign: 'right' }}>
                    {h < 24 ? hourClock(h) : ''}
                  </span>
                  <span className="flex-1 border-t" style={{ borderColor: 'var(--t-hair)', opacity: 0.6 }} />
                </div>
              )
            })}

            {/* Task workstreams */}
            <div className="absolute top-0 bottom-0" style={{ left: GUTTER, right: 6 }}>
              {laid.map((l, idx) => {
                const hue = taskHue(l.task.id, idx)
                return (
                  <TaskBand
                    key={l.task.id}
                    laid={l}
                    hue={hue}
                    winLo={win.lo}
                    pxPerMin={pxPerMin}
                    selected={selected === l.task.id}
                    dimmed={selected !== null && selected !== l.task.id}
                    onSelect={() =>
                      onSelect(
                        selected === l.task.id
                          ? null
                          : {
                              id: l.task.id,
                              day,
                              title: l.task.title,
                              minutes: l.task.minutes,
                              hue,
                              segments: l.segments,
                              summary: l.task.summary ?? [],
                              footLo: l.footLo,
                              footHi: l.footHi,
                              linkedTicket: l.task.linked_ticket,
                            },
                      )
                    }
                  />
                )
              })}
            </div>
          </div>
        </div>
      )}
      </div>
    </div>
  )
}

/** One workstream, as a single clean card spanning its footprint. A colour rail
 *  down the left edge encodes the sittings (solid where worked, a gap for each
 *  break) so the honest "worked, paused, worked" shape reads at a glance without
 *  fragmenting the card. The body is filled top-down by available height: always a
 *  title, then a meta row, then as many summary lines as fit — a tall card shows
 *  what was done rather than sitting empty. */
function TaskBand({ laid, hue, winLo, pxPerMin, selected, dimmed, onSelect }: {
  laid: LaidOutTask
  hue: string
  winLo: number
  pxPerMin: number
  selected: boolean
  dimmed: boolean
  onSelect: () => void
}) {
  const top = (laid.footLo - winLo) * pxPerMin
  const height = Math.max((laid.footHi - laid.footLo) * pxPerMin, MIN_SEG_PX)
  const left = (laid.laneIndex / laid.laneCount) * 100
  const width = 100 / laid.laneCount
  const hasBreak = laid.segments.length > 1
  const compact = height < COMPACT_MAX_PX
  const showMeta = height >= META_MIN_PX
  // How many summary lines can we show under the header without overflowing?
  const summary = laid.task.summary ?? []
  const previewLines =
    height >= SUMMARY_MIN_PX && summary.length > 0
      ? Math.max(0, Math.min(summary.length, Math.floor((height - (showMeta ? 74 : 52)) / SUMMARY_LINE_PX)))
      : 0

  return (
    <button
      onClick={e => { e.stopPropagation(); onSelect() }}
      title={laid.task.title || 'Activity'}
      className="dt-card absolute text-left transition-all"
      style={{
        top,
        height,
        left: `calc(${left}% + ${left > 0 ? 5 : 0}px)`,
        width: `calc(${width}% - ${left > 0 ? 5 : 0}px - 2px)`,
        borderRadius: 14,
        // Soft top-to-bottom tint of the task's hue — a real surface, not a hollow box.
        background: `linear-gradient(180deg, color-mix(in srgb, ${hue} ${selected ? 20 : 13}%, var(--t-card)), color-mix(in srgb, ${hue} ${selected ? 12 : 6}%, var(--t-card)))`,
        border: `1px solid color-mix(in srgb, ${hue} ${selected ? 55 : 24}%, transparent)`,
        boxShadow: selected
          ? `0 10px 30px -10px color-mix(in srgb, ${hue} 55%, transparent)`
          : `0 1px 2px rgba(0,0,0,0.04)`,
        opacity: dimmed ? 0.38 : 1,
        zIndex: selected ? 20 : 1,
        cursor: 'pointer',
        overflow: 'hidden',
      }}>
      {/* Left rail: one solid segment per worked stretch, gaps = breaks. The
          whole rail is inset from the card's top and bottom edges (RAIL_INSET)
          so it floats inside the rounded card rather than running flush into
          the corners. Interior sitting gaps keep their real positions; only the
          outermost extremes are clamped in. */}
      {laid.segments.map((s, i) => {
        // On a short card a full 10px top+bottom inset would swallow the rail,
        // so cap it at a quarter of the height — the rail stays visible and
        // still floats off both edges.
        const inset = Math.min(RAIL_INSET, height * 0.25)
        const rawTop = (s.startMin - laid.footLo) * pxPerMin
        const rawBot = (s.endMin - laid.footLo) * pxPerMin
        const top = Math.max(rawTop, inset)
        const bot = Math.min(rawBot, height - inset)
        const rh = Math.max(bot - top, 4)
        return (
          <span key={i} className="absolute" style={{
            top, height: rh, left: 7, width: 4, borderRadius: 3,
            background: hue, opacity: selected ? 1 : 0.9, pointerEvents: 'none',
          }} />
        )
      })}

      {/* Content, cleared of the rail. */}
      <div className="absolute inset-0 flex flex-col"
        style={{ padding: compact ? '0 12px 0 20px' : '9px 12px 9px 20px', justifyContent: compact ? 'center' : 'flex-start', pointerEvents: 'none' }}>
        {/* On a compact card the left rail already carries the task colour, so
            the title dot is dropped and the title is centred against the rail —
            a second dot beside a short rail just read as misaligned clutter. On
            a taller card the dot anchors the top of the header next to the rail. */}
        <div className="flex gap-2" style={{ alignItems: compact ? 'center' : 'flex-start' }}>
          {!compact && (
            <span className="shrink-0 rounded-full" style={{ width: 7, height: 7, background: hue, marginTop: 4 }} />
          )}
          <span className="mt-card-title flex-1 min-w-0"
            style={{ color: 'var(--t-title)', lineHeight: 1.25, display: '-webkit-box', WebkitLineClamp: compact ? 1 : 2, WebkitBoxOrient: 'vertical', overflow: 'hidden' }}>
            {laid.task.title || 'Activity'}
          </span>
        </div>

        {showMeta && (
          <div className="flex items-center gap-2 mt-1.5 flex-wrap" style={{ paddingLeft: 15 }}>
            {laid.task.minutes > 0 && (
              <span className="mt-mono-sm" style={{ fontSize: 10.5, fontWeight: 700, color: hue }}>
                {fmtDur(laid.task.minutes * 60)}
              </span>
            )}
            <span className="mt-mono-sm" style={{ fontSize: 10, color: 'var(--t-faint)' }}>
              {taskRangeLabel(laid)}
            </span>
            {hasBreak && (
              <span className="mt-mono-sm" style={{ fontSize: 9.5, color: 'var(--t-faint)', opacity: 0.85 }}>
                · {laid.segments.length} sittings
              </span>
            )}
          </div>
        )}

        {previewLines > 0 && (
          <div className="mt-2" style={{ paddingLeft: 15 }}>
            <Bullets items={summary.slice(0, previewLines)} accent={hue} size={11} clamp />
            {summary.length > previewLines && (
              <p className="mt-mono-sm mt-1" style={{ fontSize: 9.5, color: 'var(--t-faint)', paddingLeft: 12 }}>
                +{summary.length - previewLines} more
              </p>
            )}
          </div>
        )}
      </div>
    </button>
  )
}

function EmptyState({ isToday }: { isToday: boolean }) {
  return (
    <div className="px-6 pt-16 flex flex-col items-center text-center" style={{ color: 'var(--t-faint)' }}>
      <div className="rounded-2xl flex items-center justify-center mb-4"
        style={{ width: 52, height: 52, background: 'var(--t-box)', border: '1px solid var(--t-card-border)' }}>
        <span style={{ fontSize: 22 }}>🗂️</span>
      </div>
      <p className="mt-card-title" style={{ color: 'var(--t-muted)' }}>
        {isToday ? 'Still learning today’s tasks' : 'No tasks recorded for this day'}
      </p>
      <p className="mt-body-sm mt-1.5" style={{ maxWidth: 300, lineHeight: 1.5 }}>
        {isToday
          ? 'Meridian groups your day into a few tasks as the hours go by. Check back after your next hour of work.'
          : 'Meridian only builds tasks for days it was running and active.'}
      </p>
    </div>
  )
}
