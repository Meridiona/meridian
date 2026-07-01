//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Pure positioning + hour-bucketing math for the day timeline — no React, no
// DOM. The lane/percentage helpers use a FIXED full-day window (00:00–24:00
// local) rather than a data-derived
// span, since the timeline needs a stable frame across days with very different
// amounts of work. Vertical orientation: position is `top`/`height` (% of the
// day), not `left`/`width`.
//
// Overlap handling is new: a greedy interval-graph lane assignment (the classic
// "minimum meeting rooms" algorithm) splits same-time items into side-by-side
// columns instead of stacking them on top of each other.

import type { WorklogItem } from '@/lib/api-types'

export const HOUR_MS = 3_600_000
export const DAY_MS = 24 * HOUR_MS
const MIN_HEIGHT_PCT = 0.6 // floor so very short/point-in-time items stay visible

export interface HourTick {
  top: number // % from the top of the day
  label: string // "00".."23"
}

export interface PositionedWorklog {
  item: WorklogItem
  top: number // % of day height
  height: number // % of day height
  left: number // % of lane width
  width: number // % of lane width
}

/** Local midnight for `day` (YYYY-MM-DD), in epoch ms. */
function dayStartMs(day: string): number {
  return new Date(`${day}T00:00:00`).getTime()
}

/** Hour gridline positions + labels for a full day, top-to-bottom. */
export function hourTicks(): HourTick[] {
  const ticks: HourTick[] = []
  for (let h = 0; h <= 24; h++) {
    ticks.push({ top: (h / 24) * 100, label: String(h % 24).padStart(2, '0') })
  }
  return ticks
}

/** Group a day's worklog items into 24 buckets keyed on the local hour-of-day
 *  of their `window_start`. The one-pager timeline stacks same-hour items
 *  vertically inside a per-hour grid row (no lane-splitting), so this replaces
 *  the percentage/lane math of `layoutDay` for the new rendering strategy. Items
 *  whose `window_start` doesn't parse are dropped. Every hour 0..23 is present
 *  in the returned map (empty array when nothing falls in it) so callers can
 *  render a full-day column without gaps. */
export function bucketByHour(items: WorklogItem[]): Map<number, WorklogItem[]> {
  const buckets = new Map<number, WorklogItem[]>()
  for (let h = 0; h < 24; h++) buckets.set(h, [])
  for (const w of items) {
    const t = new Date(w.window_start)
    if (Number.isNaN(t.getTime())) continue
    buckets.get(t.getHours())!.push(w)
  }
  for (const list of buckets.values()) {
    list.sort((a, b) => new Date(a.window_start).getTime() - new Date(b.window_start).getTime())
  }
  return buckets
}

/** 12-hour clock label for an hour-of-day (`0 → "12 AM"`, `18 → "6 PM"`). */
export function hourLabel(hour: number): string {
  const period = hour >= 12 ? 'PM' : 'AM'
  const h12 = ((hour + 11) % 12) + 1
  return `${h12} ${period}`
}

interface Interval {
  item: WorklogItem
  startMs: number
  endMs: number
}

/** Clamp a worklog's window to the given day, defaulting a missing/invalid
 *  `window_end` to a minimum-visible span starting at `window_start`. */
function toInterval(w: WorklogItem, dayLo: number, dayHi: number): Interval | null {
  const startMs = new Date(w.window_start).getTime()
  if (!Number.isFinite(startMs)) return null
  const rawEnd = w.window_end ? new Date(w.window_end).getTime() : NaN
  const minSpan = (MIN_HEIGHT_PCT / 100) * DAY_MS
  const endMs = Number.isFinite(rawEnd) && rawEnd > startMs ? rawEnd : startMs + minSpan
  const clampedStart = Math.min(Math.max(startMs, dayLo), dayHi)
  const clampedEnd = Math.min(Math.max(endMs, dayLo), dayHi)
  if (clampedEnd <= clampedStart) return null
  return { item: w, startMs: clampedStart, endMs: clampedEnd }
}

/** Greedy "minimum meeting rooms" lane assignment: sort by start, give each
 *  interval the lowest-numbered lane whose last-placed end time is <= its
 *  start. Overlapping intervals end up in different lanes and share width;
 *  non-overlapping intervals collapse back to full width. */
function assignLanes(intervals: Interval[]): Map<Interval, number> {
  const sorted = [...intervals].sort((a, b) => a.startMs - b.startMs)
  const laneEnds: number[] = [] // laneEnds[i] = end time of the last item placed in lane i
  const lanes = new Map<Interval, number>()
  for (const iv of sorted) {
    let placed = false
    for (let i = 0; i < laneEnds.length; i++) {
      if (laneEnds[i] <= iv.startMs) {
        laneEnds[i] = iv.endMs
        lanes.set(iv, i)
        placed = true
        break
      }
    }
    if (!placed) {
      laneEnds.push(iv.endMs)
      lanes.set(iv, laneEnds.length - 1)
    }
  }
  return lanes
}

/** For a cluster of mutually-or-transitively overlapping intervals, the total
 *  lane count they should share width across (the max concurrent overlap). */
function clusterLaneCounts(intervals: Interval[], lanes: Map<Interval, number>): Map<Interval, number> {
  // Two intervals are in the same cluster if they overlap, transitively.
  // Union-find over the sorted-by-start list is overkill for typical per-day
  // volumes; a sweep is enough and keeps this readable.
  const sorted = [...intervals].sort((a, b) => a.startMs - b.startMs)
  const counts = new Map<Interval, number>()
  let clusterStart = 0
  let clusterMaxEnd = -Infinity
  let clusterItems: Interval[] = []

  const flush = () => {
    const laneCount = Math.max(1, ...clusterItems.map(iv => (lanes.get(iv) ?? 0) + 1))
    for (const iv of clusterItems) counts.set(iv, laneCount)
  }

  for (let i = 0; i < sorted.length; i++) {
    const iv = sorted[i]
    if (clusterItems.length === 0) {
      clusterStart = iv.startMs
      clusterMaxEnd = iv.endMs
      clusterItems = [iv]
      continue
    }
    if (iv.startMs < clusterMaxEnd) {
      clusterItems.push(iv)
      clusterMaxEnd = Math.max(clusterMaxEnd, iv.endMs)
    } else {
      flush()
      clusterStart = iv.startMs
      clusterMaxEnd = iv.endMs
      clusterItems = [iv]
    }
  }
  if (clusterItems.length > 0) flush()
  void clusterStart
  return counts
}

/** Position every worklog/proposed item for `day` on a fixed 00:00–24:00
 *  vertical timeline, with overlapping items split into side-by-side lanes. */
export function layoutDay(day: string, items: WorklogItem[]): PositionedWorklog[] {
  const dayLo = dayStartMs(day)
  const dayHi = dayLo + DAY_MS
  const intervals = items
    .map(w => toInterval(w, dayLo, dayHi))
    .filter((iv): iv is Interval => iv !== null)

  const lanes = assignLanes(intervals)
  const laneCounts = clusterLaneCounts(intervals, lanes)

  return intervals.map(iv => {
    const laneCount = laneCounts.get(iv) ?? 1
    const laneIndex = lanes.get(iv) ?? 0
    const top = ((iv.startMs - dayLo) / DAY_MS) * 100
    const height = Math.max(MIN_HEIGHT_PCT, ((iv.endMs - iv.startMs) / DAY_MS) * 100)
    return {
      item: iv.item,
      top,
      height,
      left: (laneIndex / laneCount) * 100,
      width: 100 / laneCount,
    }
  })
}
