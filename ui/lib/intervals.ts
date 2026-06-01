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

/**
 * Total wall-clock seconds covered by a set of intervals, with overlap counted
 * once. Intervals where `ended_at <= started_at` (including corrupt rows whose
 * end precedes their start) are discarded rather than contributing negative or
 * zero time.
 */
export function unionSeconds(intervals: Interval[]): number {
  const ivs = intervals
    .map(r => [new Date(r.started_at).getTime(), new Date(r.ended_at).getTime()] as [number, number])
    .filter(([s, e]) => Number.isFinite(s) && Number.isFinite(e) && e > s)
    .sort((a, b) => a[0] - b[0])

  let total = 0
  let curStart = -1
  let curEnd = -1
  for (const [s, e] of ivs) {
    if (s > curEnd) {
      if (curEnd > curStart) total += curEnd - curStart
      curStart = s
      curEnd = e
    } else if (e > curEnd) {
      curEnd = e
    }
  }
  if (curEnd > curStart) total += curEnd - curStart
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
