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
// Mostly self-contained (fetches + polls its own tasks, like OverviewPanel's plan)
// so the shell passes little more than the day. The exception is the live-hour
// strip's inputs (hourStatus/capturing/isSolo), which the shell already polls —
// see the props. Clicking a task opens a detail card with its time breakdown and
// running summary; the whole point is the day reads as a handful of workstreams
// placed where the time actually went, not 24 rows.
//
// On today, an HourTakeover strip closes the column out: the hour in progress,
// which by construction has no finished task band of its own yet. See the
// `liveMode` block for why it sits below the timeline rather than on it.

import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { fmtDur, PROVIDER_META } from '@/components/atoms'
import { ProviderIcon } from '@/components/ProviderIcon'
import { load } from '@/lib/bridge'
import type { DayTask, DayTasksResponse, HourStatus } from '@/lib/api-types'
import { HourTakeover } from './HourBadges'
import {
  layoutDayTasks,
  taskWindow,
  taskRangeLabel,
  hourClock,
  hourHasWork,
  buildTimelineScale,
  type LaidOutTask,
} from './dayTaskLayout'
import type { DayTaskDetail } from './DayTaskDetailPanel'
import { taskHue, Bullets } from './dayTaskKit'

// The vertical scale (px per minute) is DYNAMIC — the timeline fits itself to
// the pane (see buildTimelineScale in dayTaskLayout.ts), up to a comfortable
// minimum row height. Every ACTIVE clock hour gets the same honest pixel
// height as every other; a run of consecutive fully-idle hours collapses to
// one hour's worth of space, however many real hours it actually spans, so
// every visible hour-to-hour gap in the UI reads as the same size. That
// collapse is safe specifically because it only ever applies to whole hours
// with zero real work in them (see hourHasWork) — there's nothing there to
// misalign. A sparse day still grows to fill the pane; a day that's naturally
// tall even after idle collapsing simply scrolls.
const FALLBACK_PANE_PX = 560 // used before the pane is measured (first paint / SSR)
const GUTTER = 58 // px reserved on the left for hour labels
// Breathing room between the hour rail (dots/spine, which sits at the GUTTER
// edge) and the task cards, so cards read as their own column rather than
// butting up against the rail — see demo.css's dedicated .hour-spine column.
const CARD_GUTTER_GAP = 16
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

export function DayTaskColumn({ day, isToday, selectedId, onSelect, tasks, refreshToken = 0, hourStatus = [], capturing = null, isSolo = false }: {
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
  // Bumped by the shell after a dismiss/merge so this self-fetching column
  // reloads immediately (rather than waiting out the 30 s poll). Ignored on the
  // injected-tasks path, which owns its data.
  refreshToken?: number
  // The live-hour strip's inputs. Passed down rather than fetched here (this
  // column otherwise reads its own data): useTimelineData already loads and
  // polls get_hour_status for the shell, and a second poller for the same rows
  // would be pure duplication. Optional: the injected-tasks path (LLM Lab) is
  // never "today", so it has no live hour and omits them.
  hourStatus?: HourStatus[]
  /** `false` = tracking is paused RIGHT NOW. `null` while unknown. */
  capturing?: boolean | null
  isSolo?: boolean
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
  }, [day, isToday, tasks, refreshToken])

  // Two-pass layout: the window (and thus the pixel scale) depends only on the
  // tasks' real footprints, so lay out once lane-agnostically to size the column,
  // then lay out again passing the minutes the pixel floor represents at that scale
  // — that is what keeps near-touching short cards in separate side-by-side lanes.
  const laidBase = useMemo(() => layoutDayTasks(resp?.tasks ?? []), [resp])
  const win = useMemo(() => taskWindow(laidBase), [laidBase])
  const { pxPerMin, colHeight, toPx } = useMemo(
    () => buildTimelineScale(laidBase, win, paneH),
    [laidBase, win, paneH],
  )
  // A card visually occupies at least MIN_SEG_PX; add a hair of separation so
  // adjacent lanes read as distinct. Convert that pixel floor back into minutes.
  const minGapMin = (MIN_SEG_PX + CARD_SEP_PX) / pxPerMin
  const laid = useMemo(
    () => layoutDayTasks(resp?.tasks ?? [], minGapMin),
    [resp, minGapMin],
  )
  const firstHour = Math.floor(win.lo / 60)
  const lastHour = Math.ceil(win.hi / 60)
  // The current hour hasn't happened yet past `nowHour`'s own tick, so the
  // rail must never draw one past it — see the `h > firstHour && hourHasWork(...
  // h - 1)` rule below, which otherwise reads the still-open current hour as
  // "work just stopped" and ticks the NEXT hour (e.g. a "5 PM" tick at 4:51
  // PM, before 5 PM has arrived) purely because the live hour has a band.
  const nowHour = isToday ? new Date().getHours() : -1
  const tickCeiling = isToday && nowHour >= 0 ? Math.min(lastHour, nowHour) : lastHour
  // Whole-hour ticks (7 AM, 8 AM, ...) — an idle run only keeps the hour work
  // just stopped in (e.g. "1 AM" right after a 12 AM sitting ends); it does
  // NOT also keep the idle hour right before work resumes, since the resumed
  // hour's own tick already says "work picked back up here" — showing both
  // (e.g. "7 AM" AND "8 AM" back to back) is the same fact twice. The hours
  // strictly inside a run are pure repetition of "still nothing" and are
  // dropped. The same logic applies at the very end of the window: if the
  // day's last real work already stopped a few hours ago, the trailing idle
  // hours don't get a second, redundant "nothing happened" tick just because
  // one of them happens to be the last hour in the window — only firstHour is
  // unconditionally anchored, so the column always has a top label. Positions
  // come from the same buildTimelineScale as the cards, whose idle-hour
  // collapsing (see dayTaskLayout.ts) keeps every visible hour-to-hour gap
  // the same size, active or collapsed.
  const hourLines = useMemo(() => {
    const shown = new Set<number>([firstHour])
    for (let h = firstHour; h <= tickCeiling; h++) {
      if (hourHasWork(laidBase, win, h) || (h > firstHour && hourHasWork(laidBase, win, h - 1))) {
        shown.add(h)
      }
    }
    const out: { hour: number; top: number }[] = []
    for (const h of Array.from(shown).sort((a, b) => a - b)) {
      const top = toPx(h * 60)
      if (top >= -1 && top <= colHeight + 1) out.push({ hour: h, top })
    }
    return out
  }, [firstHour, tickCeiling, toPx, colHeight, laidBase, win])

  const taskCount = laid.length

  // ── The live hour ─────────────────────────────────────────────────────────
  // The current hour's status strip, restored from the old 24-row TimelineColumn
  // (which no longer renders — this column replaced it, and HourTakeover was
  // orphaned with it).
  //
  // It sits BELOW the timeline rather than at the current hour's position on it,
  // because that position cannot hold it. Two measured reasons, both live on a
  // real day:
  //   1. The current hour is usually ALREADY OCCUPIED. Work is folded into tasks
  //      continuously, so a task band is typically drawn inside the live hour
  //      right up to a minute or two ago; a takeover placed there lands on top of
  //      the card showing what is being worked on right now.
  //   2. Most of the hour has no pixel space anyway. `taskWindow` ends 20 minutes
  //      past the last segment and `toPx` CLAMPS past `win.hi` — so at 16:26 the
  //      window ended 16:22 and 38 of the hour's 60 minutes map to no pixels at
  //      all. There is no band to draw into.
  // Below the last card it reads as what it actually is: the day so far, and then
  // the hour still in progress. (`nowHour` is computed above, alongside `tickCeiling`.)
  const liveStatus = isToday ? hourStatus.find(s => s.hour === nowHour) : undefined
  // The current hour hasn't ended, so it is almost never `generating` — the DB
  // only flips that for the seconds the /worklog_hour call is in flight. `queued`
  // is what the user sees essentially always, which is exactly why the strip is
  // worth showing: it is the only thing that says the hour is being tracked at
  // all. (See HourBadges' doc comment.)
  const liveMode = !isToday ? null : liveStatus?.generating ? 'generating' as const : 'queued' as const

  return (
    // A click anywhere in the column that isn't on a card clears the selection
    // (returns the right panel to the day's glance) — cards stopPropagation so
    // clicking one still just selects/toggles it.
    <div className="relative h-full" onClick={() => { if (selected) onSelect(null) }}>
      <div ref={scrollRef} className="h-full overflow-y-auto nice-scroll">
      {/* Task count only — no date line above it (the date already lives in the
          toolbar above this column; repeating it here was redundant). */}
      <div ref={headerRef} className="px-6 pt-6 pb-2 flex items-baseline justify-between">
        <p className="mt-greeting text-title" style={{ fontSize: 20 }}>
          {taskCount > 0 ? `${taskCount} task${taskCount === 1 ? '' : 's'} today` : 'Your day, in tasks'}
        </p>
        {taskCount > 0 && (
          <p className="mt-body-sm" style={{ color: 'var(--t-faint)' }}>
            {fmtDur(laid.reduce((a, l) => a + l.task.minutes, 0) * 60)} tracked
          </p>
        )}
      </div>

      {!loaded ? null : taskCount === 0 ? (
        <EmptyState isToday={isToday} />
      ) : (
        // The live strip below supplies the bottom padding when it is there, so
        // the two don't stack into a large dead gap.
        <div className={liveMode ? 'px-6 pt-4 pb-4' : 'px-6 pt-4 pb-8'}>
          {/* pt-4: the topmost hour tick sits at colHeight-relative top:0 and is
              itself centred on its row via -translate-y-1/2 (see below), so
              without this cushion its top half is clipped by the scroll
              viewport's own edge — this is what left "8 AM" invisible. */}
          <div className="relative" style={{ height: colHeight }}>
            {/* Hour gridlines + labels — positioned via the same scale as the
                cards beneath them (see hourLines) so a label always sits at its
                real hour on the rail, decluttered for runs of hours inside one
                compressed gap segment. Ported from the marketing site's product
                demo (meridiona-website/assets/js/demo.js renderTimeline +
                assets/css/demo.css .hour-spine/.hour-node): one vertical spine
                with a node per hour — the same light ring for every ordinary
                hour, solid + pulsing accent only for the current one. The
                label itself never changes weight/color by state — only the
                node does. */}
            <span className="absolute" style={{ left: GUTTER - 5, top: 0, bottom: 0, width: 2, background: 'var(--t-hair)', opacity: 0.22 }} />
            {hourLines.map(({ hour, top }) => {
              const isNow = isToday && hour === nowHour
              return (
                <div key={hour} className="absolute left-0 right-0 flex items-start"
                  style={{ top, height: 0 }}>
                  {/* One-off: real JetBrains Mono (--font-jetbrains-mono, layout.tsx),
                      not the app's --font-mono alias — matches the reference demo's
                      .hour-label exactly, scoped to this rail only. */}
                  <span className="shrink-0 -translate-y-1/2"
                    style={{
                      width: GUTTER - 12, fontSize: 10.5, fontWeight: 600, textAlign: 'right',
                      // paddingRight: breathing room before the dot/spine — without it
                      // the right-aligned text sits almost flush against the node.
                      paddingRight: 6,
                      color: 'var(--t-faint-2)', fontFamily: 'var(--font-jetbrains-mono), monospace',
                    }}>
                    {/* Blank for the live hour — the takeover strip below owns both
                        the label AND the live dot now (see liveMode's render below),
                        so this row isn't left with an orphaned pulsing dot up here
                        with nothing beside it. The gridline still draws (next span)
                        since this row can mark a real boundary — the previous hour's
                        work ending — just without repeating the hour marker twice. */}
                    {isNow ? '' : hourClock(hour)}
                  </span>
                  {/* Every ordinary hour gets the same light ring: panel fill inside,
                      a thin muted border outside. The live hour's own accent+pulse
                      dot now lives next to its label in the takeover strip instead
                      of up here (see above), so nothing renders in its place. */}
                  {!isNow && (
                    <span className="absolute rounded-full -translate-y-1/2" style={{
                      left: GUTTER - 5 - 4.5, width: 9, height: 9,
                      background: 'var(--t-panel)',
                      border: '2px solid color-mix(in srgb, var(--t-faint-2) 55%, transparent)',
                      boxShadow: '0 0 0 3px var(--t-panel)',
                    }} />
                  )}
                  {/* Starts past the card gutter (not GUTTER) so it never touches the
                      spine/node — a separate row divider for the card column only,
                      same as demo.css's .hour-body border-top living in its own
                      grid column rather than the spine's. */}
                  <span className="flex-1 border-t" style={{ borderColor: 'var(--t-hair)', opacity: 0.5, marginLeft: 12 + CARD_GUTTER_GAP }} />
                </div>
              )
            })}

            {/* Task workstreams — inset further than the rail itself (GUTTER)
                so the cards read as their own column, not flush against the
                dots/spine (see demo.css's dedicated 26px .hour-spine column). */}
            <div className="absolute top-0 bottom-0" style={{ left: GUTTER + CARD_GUTTER_GAP, right: 6 }}>
              {laid.map((l, idx) => {
                const hue = taskHue(l.task.id, idx)
                return (
                  <TaskBand
                    key={l.task.id}
                    laid={l}
                    hue={hue}
                    toPx={toPx}
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

      {/* The hour in progress. Rendered on today only, and on the empty day too
          — a day with nothing folded yet is exactly when "Meridian is watching
          this hour" is the most useful thing on the screen. The gutter reserves
          the same label width as the gridlines above (GUTTER - 12, then 12px) so
          the strip lines up with the task cards. The hour text is always drawn
          here (even when the grid above already ticked this hour) — leaving it
          blank made the card read as floating/unaligned, with nothing to its
          left tying it to the current hour; a little label duplication reads
          better than that. items-start so the label's top lines up with the
          card's own top edge (its border), not the vertical center of the
          whole card, which sat noticeably lower than where the card starts. */}
      {loaded && liveMode && (
        <div className="px-6 pb-8 flex items-start">
          <span className="mt-mono-sm shrink-0" style={{
            width: GUTTER - 12, fontSize: 10.5, textAlign: 'right', paddingRight: 6,
            color: 'var(--color-state-pending)',
          }}>
            {hourClock(nowHour)}
          </span>
          {/* The live-hour dot the grid above no longer draws (moved here so it
              sits next to the current-hour label instead of floating alone with
              nothing beside it) — same accent+pulse treatment as an ordinary
              hour node's `isNow` state used to get up on the rail. 4px gap on
              each side, taken out of the card's own marginLeft below so the
              card's visible edge doesn't shift from where it sat before. */}
          <span className="shrink-0 rounded-full live-dot" style={{
            width: 9, height: 9, marginLeft: 4,
            background: 'var(--color-state-proposal)',
            boxShadow: '0 0 0 3px color-mix(in srgb, var(--color-state-proposal) 18%, transparent)',
          }} />
          {/* 12px carries the label's GUTTER - 12 width up to GUTTER, then
              CARD_GUTTER_GAP up to where the task cards actually start —
              less HourTakeover's own mx-2, so its visible edge lines up with
              the cards above it rather than sitting further left; less the
              dot's own width + both 4px gaps, now that it sits inline here too. */}
          <div className="flex-1 min-w-0" style={{ marginLeft: 12 + CARD_GUTTER_GAP - 8 - 9 - 4 - 4 }}>
            <HourTakeover
              hour={nowHour}
              mode={liveMode}
              paused={capturing === false}
              nextHourLabel={hourClock(nowHour + 1)}
              isSolo={isSolo}
            />
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
/** The "synced to {tracker}" pill on a posted task card: the tracker's brand mark
 *  + a check. Sits in-flow at the end of the title row so it scales and is never
 *  clipped by the card's rounded corner. The pill's own background is the card's
 *  surface color (not a green tint) with a crisp white ring around it, so it reads
 *  as a badge sitting ON the card rather than a colored chip competing with it —
 *  only the check mark itself stays approved-green, as the one "done" signal. */
function PostedPill({ provider, targetKey, alignTop }: { provider: string; targetKey: string | null; alignTop: boolean }) {
  const label = PROVIDER_META[provider]?.label ?? provider
  return (
    <span
      className="shrink-0 inline-flex items-center gap-1 rounded-full"
      title={`Synced to ${label}${targetKey ? ` · ${targetKey}` : ''}`}
      style={{
        alignSelf: alignTop ? 'flex-start' : 'center',
        marginTop: alignTop ? 1 : 0,
        padding: '2.5px 7px 2.5px 6px',
        background: 'var(--t-card)',
        border: '1.5px solid #fff',
        boxShadow: '0 1px 3px rgba(0,0,0,0.12)',
      }}>
      <ProviderIcon provider={provider} size={11} />
      <svg width="9" height="9" viewBox="0 0 24 24" fill="none" aria-hidden
        style={{ color: 'var(--color-state-approved)' }}>
        <path d="M20 6 9 17l-5-5" stroke="currentColor" strokeWidth="3.5" strokeLinecap="round" strokeLinejoin="round" />
      </svg>
    </span>
  )
}

function TaskBand({ laid, hue, toPx, selected, dimmed, onSelect }: {
  laid: LaidOutTask
  hue: string
  toPx: (min: number) => number
  selected: boolean
  dimmed: boolean
  onSelect: () => void
}) {
  const cardTop = toPx(laid.footLo)
  const height = Math.max(toPx(laid.footHi) - cardTop, MIN_SEG_PX)
  const left = (laid.laneIndex / laid.laneCount) * 100
  const width = 100 / laid.laneCount
  const hasBreak = laid.segments.length > 1
  const compact = height < COMPACT_MAX_PX
  const showMeta = height >= META_MIN_PX
  // How many summary lines can we show under the header without overflowing?
  // If not every line fits, one of those slots has to go to the "+N more"
  // caption itself — otherwise the caption is an EXTRA row tacked on below
  // however many bullets fit exactly, pushing the card past its own height
  // and getting silently clipped by overflow:hidden (a half-cut-off bullet
  // line with no "+more" to explain it).
  //
  // The header's real height varies (a long title wraps to 2 lines, a short
  // one sits on 1), so a flat 52/74px guess under-budgets a wrapped title —
  // that's what let a wrapped-title card overflow silently with no "+N more"
  // (the slot math thought there was room that the actual DOM didn't have).
  // headerRef measures the real title+meta block instead of guessing.
  const headerRef = useRef<HTMLDivElement>(null)
  const [headerH, setHeaderH] = useState<number | null>(null)
  useLayoutEffect(() => {
    const el = headerRef.current
    if (!el) return
    const measure = () => setHeaderH(el.offsetHeight)
    measure()
    const ro = new ResizeObserver(measure)
    ro.observe(el)
    return () => ro.disconnect()
  }, [laid.task.title, laid.task.posted_provider, showMeta, compact])
  const summary = laid.task.summary ?? []
  const GAP_ABOVE_SUMMARY_PX = 8 // the summary block's own mt-2
  const summarySlots = height >= SUMMARY_MIN_PX
    ? Math.floor((height - (headerH ?? (showMeta ? 74 : 52)) - GAP_ABOVE_SUMMARY_PX) / SUMMARY_LINE_PX)
    : 0
  const previewLines =
    summary.length === 0
      ? 0
      : summarySlots >= summary.length
        ? summary.length // everything fits — no caption needed
        : Math.max(0, Math.min(summary.length, summarySlots - 1)) // reserve a slot for "+N more"

  return (
    <button
      onClick={e => { e.stopPropagation(); onSelect() }}
      title={laid.task.title || 'Activity'}
      className="dt-card absolute text-left transition-all"
      style={{
        top: cardTop,
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
        const rawTop = toPx(s.startMin) - cardTop
        const rawBot = toPx(s.endMin) - cardTop
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
        <div ref={headerRef}>
          <div className="flex gap-2" style={{ alignItems: compact ? 'center' : 'flex-start' }}>
            {!compact && (
              <span className="shrink-0 rounded-full" style={{ width: 7, height: 7, background: hue, marginTop: 4 }} />
            )}
            <span className="mt-card-title flex-1 min-w-0"
              style={{ color: 'var(--t-title)', lineHeight: 1.25, display: '-webkit-box', WebkitLineClamp: compact ? 1 : 2, WebkitBoxOrient: 'vertical', overflow: 'hidden' }}>
              {laid.task.title || 'Activity'}
            </span>
            {/* "Synced to {tracker}" pill — shown once this task's worklog is posted,
                so the timeline itself flags which cards are on the PM board. In-flow
                at the row's end (never clipped by the card's rounded corner) and
                vertically centred on compact cards. Non-interactive; the detail
                panel carries the clickable link. */}
            {laid.task.posted_provider && (
              <PostedPill
                provider={laid.task.posted_provider}
                targetKey={laid.task.posted_target_key}
                alignTop={!compact}
              />
            )}
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
        </div>

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
