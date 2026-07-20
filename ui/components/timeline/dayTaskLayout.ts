//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Pure positioning math for the day-task timeline — no React, no DOM. A task is
// drawn from its SEGMENTS: the approximate "HH:MM-HH:MM" stretches it was worked
// (migration 059). Multiple segments = breaks, so a task worked 08:01-08:59 then
// 09:31-09:39 draws as two solid bars inside one lane, with the gap between them
// reading as a break in the same workstream. Everything here is minute-precise
// (0..1440), not snapped to whole hours. Tasks whose footprints overlap in time
// are split into side-by-side lanes so parallel work still reads clearly.
//
// A pre-059 row with no segments falls back to its whole-hour span so the timeline
// keeps rendering during the rollout.

import type { DayTask } from '@/lib/api-types'

/** One worked stretch, in minutes-from-midnight `[startMin, endMin)`. */
export interface LaidSegment {
  startMin: number
  endMin: number
}

export interface LaidOutTask {
  task: DayTask
  segments: LaidSegment[] // ascending, coalesced; the solid bars
  footLo: number          // earliest segment start (minutes) — the lane footprint
  footHi: number          // latest segment end (minutes)
  laneIndex: number       // 0-based column within its overlap cluster
  laneCount: number       // columns to share width across in that cluster
}

/** `"HH:MM"` -> minutes-from-midnight (`"08:15"` -> 495, `"24:00"` -> 1440).
 *  `null` for anything malformed. */
export function hhmmToMin(s: string): number | null {
  const m = /^(\d{1,2}):(\d{2})$/.exec(s.trim())
  if (!m) return null
  const h = Number(m[1])
  const min = Number(m[2])
  if (min < 0 || min > 59) return null
  if (h === 24 && min === 0) return 1440
  if (h < 0 || h > 23) return null
  return h * 60 + min
}

/** Minutes-from-midnight -> a 12-hour clock label (`495 -> "8:15 AM"`). */
export function clockLabel(min: number): string {
  const clamped = Math.max(0, Math.min(1440, Math.round(min)))
  const hod = Math.floor(clamped / 60) % 24
  const mm = clamped % 60
  const period = hod >= 12 ? 'PM' : 'AM'
  const h12 = ((hod + 11) % 12) + 1
  return `${h12}:${String(mm).padStart(2, '0')} ${period}`
}

/** An RFC-3339 timestamp -> a local 12-hour clock label (`"2:14 PM"`), or `''`
 *  for an unparsable value — callers show nothing rather than "Invalid Date". */
export function clockLabelFromIso(iso: string): string {
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return ''
  return clockLabel(d.getHours() * 60 + d.getMinutes())
}

/** 12-hour label for a whole hour-of-day (`0 -> "12 AM"`, `18 -> "6 PM"`). */
export function hourClock(hour: number): string {
  const hod = ((hour % 24) + 24) % 24
  const period = hod >= 12 ? 'PM' : 'AM'
  const h12 = ((hod + 11) % 12) + 1
  return `${h12} ${period}`
}

/** The whole task's approximate time range as a single readable label, or `''`. */
export function taskRangeLabel(l: LaidOutTask): string {
  if (l.segments.length === 0) return ''
  return `${clockLabel(l.footLo)} - ${clockLabel(l.footHi)}`
}

/** The smallest window we ever draw, in minutes. A sparse day (one short task,
 *  the first hour after a fresh fold) would otherwise collapse to a few unreadable
 *  pixels; enforcing a floor keeps a task centred in real surrounding-hour context
 *  and lets the column fill the pane. */
export const MIN_WINDOW_MIN = 150

/** The minute window [lo, hi) the laid-out tasks occupy, padded ~20 min each side,
 *  widened to at least `MIN_WINDOW_MIN` (centred on the work) so a sparse day still
 *  reads, and clamped to the day. A sensible working-hours default when empty. */
export function taskWindow(laid: LaidOutTask[]): { lo: number; hi: number } {
  if (laid.length === 0) return { lo: 8 * 60, hi: 19 * 60 }
  let lo = Math.min(...laid.map(l => l.footLo)) - 20
  let hi = Math.max(...laid.map(l => l.footHi)) + 20
  if (hi - lo < MIN_WINDOW_MIN) {
    const mid = (lo + hi) / 2
    lo = mid - MIN_WINDOW_MIN / 2
    hi = mid + MIN_WINDOW_MIN / 2
  }
  return { lo: Math.max(0, Math.floor(lo)), hi: Math.min(1440, Math.ceil(hi)) }
}

// ── Time→pixel scale ─────────────────────────────────────────────────────────
// The column always starts from one fixed, honest rate — MIN_PX_PER_MIN — which
// is what "real time" means on this timeline: a full/busy day drawn at that rate
// has a real size, so if it's taller than the pane it genuinely overflows and
// scrolls (a `clamp(paneH / windowMin)` formula looks equivalent but isn't:
// whenever unclamped it SOLVES pxPerMin so colHeight lands exactly on paneH by
// construction, i.e. it can never overflow — this bit us before).
//
// But a day can ALSO be genuinely short at that rate — a lone short task, or
// the first hour after a fresh fold — and a short day sitting in a tall pane
// with a dead void underneath it looks broken. So: compute the natural
// (honest, real-time) size first; if it's shorter than the pane, grow the
// WHOLE thing by one uniform factor until it fills the pane, capped at
// MAX_PX_PER_MIN so a lone very-short task doesn't get stretched to something
// absurd. A day that's already taller than the pane is left alone and just
// scrolls. The rate is always ONE constant across the whole column — never
// compressed differently in some stretches than others — so an hour is
// always the same height as every other hour: aligned with whatever's drawn
// against it, and evenly spaced from its neighbours, by construction.
export const MIN_PX_PER_MIN = 1 // 60 px/hr — the one real, honest rate everything starts from
export const MAX_PX_PER_MIN = 9 // ~540 px/hr — how far a short day can grow to fill a tall pane

/** How much to uniformly grow a `naturalHeight` (already rendered at
 *  MIN_PX_PER_MIN) to fill `paneH`, capped so growth never exceeds
 *  MAX_PX_PER_MIN/MIN_PX_PER_MIN. 1 (no growth) once it already fills or
 *  exceeds the pane. */
function growToFillFactor(naturalHeight: number, paneH: number): number {
  if (naturalHeight <= 0 || naturalHeight >= paneH) return 1
  return Math.min(paneH / naturalHeight, MAX_PX_PER_MIN / MIN_PX_PER_MIN)
}

/** The column's uniform pixels-per-minute rate and total height for a window
 *  of `windowMin` minutes in a pane of `paneH` px. See the block comment above. */
export function timelineScale(windowMin: number, paneH: number): { pxPerMin: number; colHeight: number } {
  const naturalHeight = windowMin * MIN_PX_PER_MIN
  const factor = growToFillFactor(naturalHeight, paneH)
  const pxPerMin = MIN_PX_PER_MIN * factor
  return { pxPerMin, colHeight: windowMin * pxPerMin }
}

/** Does ANY task have real work during clock hour `hour` (clamped to `win`)?
 *  The single source of truth for "is this hour idle", shared by the hour-tick
 *  label filter (DayTaskColumn) and the collapsed-idle scale below, so the two
 *  never disagree about which hours are empty. */
export function hourHasWork(laid: LaidOutTask[], win: { lo: number; hi: number }, hour: number): boolean {
  const loMin = Math.max(hour * 60, win.lo)
  const hiMin = Math.min((hour + 1) * 60, win.hi)
  return hiMin > loMin && laid.some(l => l.segments.some(s => s.startMin < hiMin && s.endMin > loMin))
}

/** Builds the column's scale, treating a run of consecutive fully-idle whole
 *  clock hours as ONE collapsed step — the same size as a single real hour —
 *  however many real hours it actually spans. This is the one place idle time
 *  is ever compressed, and it's safe specifically because it operates at
 *  WHOLE-HOUR granularity: an hour classified idle here has zero real work
 *  anywhere inside it (see hourHasWork), so nothing is ever drawn in the
 *  compressed space and no card's rail can drift out of alignment with its
 *  label — unlike compressing raw sub-minute gaps, which can have a brief
 *  real sitting hidden inside what looks like one continuous gap.
 *
 *  Every ACTIVE hour still draws at the one honest MIN_PX_PER_MIN rate, so a
 *  card's position within it is exact. The whole result then grows to fill
 *  `paneH` if it's naturally shorter (see growToFillFactor); a day that's
 *  already tall enough just scrolls. */
export function buildTimelineScale(
  laid: LaidOutTask[],
  win: { lo: number; hi: number },
  paneH: number,
): { pxPerMin: number; colHeight: number; toPx: (min: number) => number } {
  const firstHour = Math.floor(win.lo / 60)
  const lastHour = Math.ceil(win.hi / 60)

  // Whole-hour buckets, merging consecutive hours of the same kind.
  const buckets: { active: boolean; loMin: number; hiMin: number }[] = []
  for (let h = firstHour; h < lastHour; h++) {
    const loMin = Math.max(h * 60, win.lo)
    const hiMin = Math.min((h + 1) * 60, win.hi)
    if (hiMin <= loMin) continue
    const active = hourHasWork(laid, win, h)
    const prev = buckets[buckets.length - 1]
    if (prev && prev.active === active) prev.hiMin = hiMin
    else buckets.push({ active, loMin, hiMin })
  }
  if (buckets.length === 0) buckets.push({ active: true, loMin: win.lo, hiMin: win.hi })

  const idleStepPx = 60 * MIN_PX_PER_MIN // one real hour's worth — what every collapsed idle run is capped to, active or not

  // Pass 1: natural size at the fixed floor rate, idle runs capped to one hour.
  const natural: (typeof buckets[number] & { loPx: number; hiPx: number })[] = []
  let naturalPx = 0
  for (const b of buckets) {
    const uncapped = (b.hiMin - b.loMin) * MIN_PX_PER_MIN
    const h = b.active ? uncapped : Math.min(uncapped, idleStepPx)
    natural.push({ ...b, loPx: naturalPx, hiPx: naturalPx + h })
    naturalPx += h
  }

  // Pass 2: grow everything by one uniform factor to fill the pane if there's slack.
  const factor = growToFillFactor(naturalPx, paneH)
  const pxPerMin = MIN_PX_PER_MIN * factor
  const segments = factor === 1 ? natural : natural.map(s => ({ ...s, loPx: s.loPx * factor, hiPx: s.hiPx * factor }))
  const colHeight = naturalPx * factor

  const toPx = (min: number): number => {
    const clamped = Math.max(win.lo, Math.min(win.hi, min))
    for (const seg of segments) {
      if (clamped >= seg.loMin && clamped <= seg.hiMin) {
        const span = seg.hiMin - seg.loMin
        const t = span > 0 ? (clamped - seg.loMin) / span : 0
        return seg.loPx + t * (seg.hiPx - seg.loPx)
      }
    }
    return segments.length > 0 ? segments[segments.length - 1].hiPx : 0
  }

  return { pxPerMin, colHeight, toPx }
}

/** Parse a task's segments to minute ranges. Falls back to the whole-hour span for
 *  a pre-059 row with no segments. `null` if the task covers no time at all. */
function toSegments(t: DayTask): LaidSegment[] | null {
  const segs: LaidSegment[] = []
  for (const s of t.segments ?? []) {
    const a = hhmmToMin(s.start)
    const b = hhmmToMin(s.end)
    if (a === null || b === null || b <= a) continue
    segs.push({ startMin: a, endMin: b })
  }
  if (segs.length > 0) {
    segs.sort((x, y) => x.startMin - y.startMin)
    return segs
  }
  // Pre-059 fallback: draw the coarse whole-hour span.
  if (t.first_hour >= 0 && t.last_hour >= 0) {
    return [{ startMin: t.first_hour * 60, endMin: (t.last_hour + 1) * 60 }]
  }
  return null
}

interface Foot {
  task: DayTask
  segments: LaidSegment[]
  lo: number
  hi: number
}

/** A footprint's *effective* end for collision purposes: never shorter than
 *  `minGapMin` past its start. A few-minute task is drawn at a pixel floor
 *  (`MIN_SEG_PX`) that represents more time than it truly spans, so two tasks
 *  whose real times merely touch would still paint on top of each other in one
 *  lane. Widening the end by the floor pushes such a pair into side-by-side lanes,
 *  guaranteeing the cards never visually overlap. */
function effEnd(f: Foot, minGapMin: number): number {
  return Math.max(f.hi, f.lo + minGapMin)
}

/** Greedy lane assignment on task footprints: sort by start, give each the lowest
 *  lane whose last (effective) end is <= its start. All of a task's segments stay
 *  in one lane. `minGapMin` accounts for the minimum drawn height (see effEnd). */
function assignLanes(feet: Foot[], minGapMin: number): Map<Foot, number> {
  const sorted = [...feet].sort((a, b) => a.lo - b.lo)
  const laneEnds: number[] = []
  const lanes = new Map<Foot, number>()
  for (const f of sorted) {
    let placed = false
    for (let i = 0; i < laneEnds.length; i++) {
      if (laneEnds[i] <= f.lo) {
        laneEnds[i] = effEnd(f, minGapMin)
        lanes.set(f, i)
        placed = true
        break
      }
    }
    if (!placed) {
      laneEnds.push(effEnd(f, minGapMin))
      lanes.set(f, laneEnds.length - 1)
    }
  }
  return lanes
}

/** For each cluster of transitively-overlapping footprints, the lane count to
 *  share width across (the max concurrent overlap in that cluster). Overlap is
 *  measured with the same `minGapMin` buffer used for lane assignment. */
function clusterLaneCounts(feet: Foot[], lanes: Map<Foot, number>, minGapMin: number): Map<Foot, number> {
  const sorted = [...feet].sort((a, b) => a.lo - b.lo)
  const counts = new Map<Foot, number>()
  let clusterMaxEnd = -Infinity
  let clusterItems: Foot[] = []

  const flush = () => {
    const laneCount = Math.max(1, ...clusterItems.map(f => (lanes.get(f) ?? 0) + 1))
    for (const f of clusterItems) counts.set(f, laneCount)
  }

  for (const f of sorted) {
    if (clusterItems.length === 0) {
      clusterMaxEnd = effEnd(f, minGapMin)
      clusterItems = [f]
      continue
    }
    if (f.lo < clusterMaxEnd) {
      clusterItems.push(f)
      clusterMaxEnd = Math.max(clusterMaxEnd, effEnd(f, minGapMin))
    } else {
      flush()
      clusterMaxEnd = effEnd(f, minGapMin)
      clusterItems = [f]
    }
  }
  if (clusterItems.length > 0) flush()
  return counts
}

/** Lay out every task from its segments, assigning a lane per overlap cluster.
 *  Tasks with no time are dropped. Ordered by start so render order is stable.
 *  `minGapMin` (minutes the pixel floor represents at the current scale) makes
 *  near-touching short tasks split into side-by-side lanes so cards never overlap. */
export function layoutDayTasks(tasks: DayTask[], minGapMin = 0): LaidOutTask[] {
  const feet: Foot[] = tasks
    .map(t => {
      const segments = toSegments(t)
      if (!segments) return null
      return {
        task: t,
        segments,
        lo: segments[0].startMin,
        hi: Math.max(...segments.map(s => s.endMin)),
      }
    })
    .filter((f): f is Foot => f !== null)
    .sort((a, b) => a.lo - b.lo || a.task.id.localeCompare(b.task.id))

  const lanes = assignLanes(feet, minGapMin)
  const laneCounts = clusterLaneCounts(feet, lanes, minGapMin)

  return feet.map(f => ({
    task: f.task,
    segments: f.segments,
    footLo: f.lo,
    footHi: f.hi,
    laneIndex: lanes.get(f) ?? 0,
    laneCount: laneCounts.get(f) ?? 1,
  }))
}
