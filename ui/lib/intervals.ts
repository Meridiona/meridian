// meridian — normalises screenpipe activity into structured app sessions
//
// Wall-clock interval math shared across the dashboard. Meridian stores two
// overlapping recordings of the same time in `app_sessions`: the screen-capture
// stream (foreground app) and the coding-agent transcript stream (Claude Code /
// Codex). SUMMING their durations double-counts every overlapping second, so any
// "total time" figure must UNION intervals instead. These helpers are the single
// source of that math — used by the Today and coding-agents routes.

export interface Interval {
  started_at: string
  ended_at: string
}

type Pair = [number, number] // [startMs, endMs]

/**
 * Parse, drop invalid intervals (`end <= start`, including corrupt rows whose
 * end precedes their start, and unparseable timestamps), sort by start, and
 * merge overlapping/touching intervals into a disjoint, ascending set.
 */
function normalize(intervals: Interval[]): Pair[] {
  const ivs = intervals
    .map(r => [new Date(r.started_at).getTime(), new Date(r.ended_at).getTime()] as Pair)
    .filter(([s, e]) => Number.isFinite(s) && Number.isFinite(e) && e > s)
    .sort((a, b) => a[0] - b[0])

  const out: Pair[] = []
  for (const [s, e] of ivs) {
    const last = out[out.length - 1]
    if (last && s <= last[1]) last[1] = Math.max(last[1], e)
    else out.push([s, e])
  }
  return out
}

/**
 * Total wall-clock seconds covered by a set of intervals, with overlap counted
 * once. Invalid/corrupt intervals contribute nothing.
 */
export function unionSeconds(intervals: Interval[]): number {
  const merged = normalize(intervals)
  let total = 0
  for (const [s, e] of merged) total += e - s
  return Math.round(total / 1000)
}

/**
 * Merge a set of intervals into a disjoint, ascending list — the timeline's
 * presence/agent bands are drawn from these so overlapping rows render as one
 * continuous block rather than stacked duplicates.
 */
export function mergeIntervals(intervals: Interval[]): Interval[] {
  return normalize(intervals).map(([s, e]) => ({
    started_at: new Date(s).toISOString(),
    ended_at: new Date(e).toISOString(),
  }))
}

/**
 * Total seconds where set `a` and set `b` overlap — e.g. agent time that fell
 * inside foreground-active time (supervised / "AI-assisted") vs outside it
 * (autonomous). Both sides are normalized first, then swept together in linear
 * time.
 */
export function intersectSeconds(a: Interval[], b: Interval[]): number {
  const A = normalize(a)
  const B = normalize(b)
  let i = 0
  let j = 0
  let total = 0
  while (i < A.length && j < B.length) {
    const lo = Math.max(A[i][0], B[j][0])
    const hi = Math.min(A[i][1], B[j][1])
    if (hi > lo) total += hi - lo
    // advance whichever interval ends first
    if (A[i][1] < B[j][1]) i++
    else j++
  }
  return Math.round(total / 1000)
}

/**
 * Count genuine context switches in a time-ordered foreground stream: the number
 * of times the active app changes between consecutive sessions. Sessions shorter
 * than `minDurationS` are dropped first, so the sub-second foreground flicker
 * screenpipe emits (rapid Finder↔Chrome focus jitter) is not mistaken for the
 * user actually switching contexts.
 */
export function countSwitches(
  sessions: Array<{ app: string; started_at: string; dur: number }>,
  minDurationS: number,
): number {
  const ordered = sessions
    .filter(s => s.dur >= minDurationS)
    .sort((a, b) => new Date(a.started_at).getTime() - new Date(b.started_at).getTime())

  let switches = 0
  for (let i = 1; i < ordered.length; i++) {
    if (ordered[i].app !== ordered[i - 1].app) switches++
  }
  return switches
}
