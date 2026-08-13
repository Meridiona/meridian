//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, it, expect } from 'bun:test'
import {
  hhmmToMin,
  clockLabel,
  taskWindow,
  taskRangeLabel,
  layoutDayTasks,
  timelineScale,
  buildTimelineScale,
  hourHasWork,
  MIN_PX_PER_MIN,
  MAX_PX_PER_MIN,
  MIN_WINDOW_MIN,
} from '../components/timeline/dayTaskLayout'
import type { DayTask } from '../lib/api-types'

// Minimal DayTask builder — only the fields the layout reads.
function task(
  id: string,
  segments: { start: string; end: string }[],
  extra: Partial<DayTask> = {},
): DayTask {
  return {
    id,
    title: `Task ${id}`,
    summary: [],
    minutes: 0,
    hours: [],
    segments,
    first_hour: -1,
    last_hour: -1,
    status: 'active',
    linked_ticket: null,
    // Required on `DayTask`, and not optional there on purpose: the reader always
    // sends them (null when nothing is posted). Omitting them here let the
    // `Partial<DayTask>` spread widen them to `| undefined`, which is a state the
    // real payload never has - so the fixture was typed looser than the thing it
    // stands in for.
    posted_provider: null,
    posted_target_key: null,
    posted_browse_url: null,
    ...extra,
  }
}

describe('hhmmToMin', () => {
  it('parses HH:MM to minutes-from-midnight', () => {
    expect(hhmmToMin('08:15')).toBe(495)
    expect(hhmmToMin('00:00')).toBe(0)
    expect(hhmmToMin('24:00')).toBe(1440)
  })
  it('rejects malformed / out-of-range', () => {
    expect(hhmmToMin('8h15')).toBeNull()
    expect(hhmmToMin('25:00')).toBeNull()
    expect(hhmmToMin('08:75')).toBeNull()
  })
})

describe('clockLabel', () => {
  it('renders a 12-hour clock label', () => {
    expect(clockLabel(495)).toBe('8:15 AM')
    expect(clockLabel(0)).toBe('12:00 AM')
    expect(clockLabel(13 * 60 + 5)).toBe('1:05 PM')
  })
})

describe('layoutDayTasks', () => {
  it('keeps a task’s multiple segments in one lane (breaks read as one workstream)', () => {
    // Worked 08:01-08:59, then 09:31-09:39 — one task, one lane, two bars.
    const laid = layoutDayTasks([
      task('T1', [
        { start: '08:01', end: '08:59' },
        { start: '09:31', end: '09:39' },
      ]),
    ])
    expect(laid).toHaveLength(1)
    expect(laid[0].segments).toHaveLength(2)
    expect(laid[0].footLo).toBe(481) // 08:01
    expect(laid[0].footHi).toBe(579) // 09:39
    expect(laid[0].laneCount).toBe(1)
    expect(taskRangeLabel(laid[0])).toBe('8:01 AM - 9:39 AM')
  })

  it('splits time-overlapping tasks into side-by-side lanes', () => {
    const laid = layoutDayTasks([
      task('T1', [{ start: '08:00', end: '10:00' }]),
      task('T2', [{ start: '09:00', end: '11:00' }]),
    ])
    expect(laid).toHaveLength(2)
    expect(laid.every(l => l.laneCount === 2)).toBe(true)
    expect(new Set(laid.map(l => l.laneIndex))).toEqual(new Set([0, 1]))
  })

  it('gives non-overlapping tasks a single shared lane', () => {
    const laid = layoutDayTasks([
      task('T1', [{ start: '08:00', end: '09:00' }]),
      task('T2', [{ start: '10:00', end: '11:00' }]),
    ])
    expect(laid.every(l => l.laneCount === 1)).toBe(true)
  })

  it('falls back to the whole-hour span for a pre-059 row with no segments', () => {
    const laid = layoutDayTasks([task('T1', [], { first_hour: 8, last_hour: 10 })])
    expect(laid).toHaveLength(1)
    expect(laid[0].footLo).toBe(8 * 60)
    expect(laid[0].footHi).toBe(11 * 60) // last_hour + 1
  })

  it('drops a task that covers no time', () => {
    expect(layoutDayTasks([task('T1', [])])).toHaveLength(0)
  })

  it('separates near-touching short tasks into side-by-side lanes (no visual overlap)', () => {
    // Real times barely miss (08:00-08:02, then 08:03-08:05) so at a fine scale
    // they'd share one lane — but each is drawn at a pixel floor that spans ~10 min
    // here, so they must split. minGapMin models that floor.
    const tasks = [
      task('T1', [{ start: '08:00', end: '08:02' }]),
      task('T2', [{ start: '08:03', end: '08:05' }]),
    ]
    expect(layoutDayTasks(tasks, 0).every(l => l.laneCount === 1)).toBe(true) // no floor → one lane
    const laid = layoutDayTasks(tasks, 10) // floor spans 10 min → must separate
    expect(laid.every(l => l.laneCount === 2)).toBe(true)
    expect(new Set(laid.map(l => l.laneIndex))).toEqual(new Set([0, 1]))
  })

  it('ignores malformed segments', () => {
    const laid = layoutDayTasks([
      task('T1', [
        { start: '09:00', end: '08:00' }, // end before start
        { start: '10:00', end: '10:30' }, // valid
      ]),
    ])
    expect(laid[0].segments).toEqual([{ startMin: 600, endMin: 630 }])
  })
})

describe('taskWindow', () => {
  it('pads ~20 min each side for a wide-enough span', () => {
    // 08:00-10:00 → 120 min raw; +20 each side = 160 min ≥ MIN_WINDOW_MIN, no widening.
    const laid = layoutDayTasks([task('T1', [{ start: '08:00', end: '10:00' }])])
    const win = taskWindow(laid)
    expect(win.lo).toBe(8 * 60 - 20) // 460
    expect(win.hi).toBe(10 * 60 + 20) // 620
  })
  it('widens a sparse day to the minimum window, centred on the work', () => {
    // 08:10-09:00 → padded span is 90 min < 150; widen to 150 centred on mid 08:35.
    const laid = layoutDayTasks([task('T1', [{ start: '08:10', end: '09:00' }])])
    const win = taskWindow(laid)
    expect(win.hi - win.lo).toBe(150)
    expect((win.lo + win.hi) / 2).toBe(8 * 60 + 35) // 515
  })
  it('has a working-hours default when empty', () => {
    expect(taskWindow([])).toEqual({ lo: 480, hi: 1140 })
  })
})

describe('timelineScale', () => {
  it('leaves a day already taller than the pane at the floor rate, so it scrolls', () => {
    // This is the actual regression case: a `clamp(paneH / windowMin)` formula
    // solves pxPerMin so colHeight always lands exactly on paneH, so a normal
    // day never overflows and never scrolls, no matter how the bounds are tuned.
    const windowMin = 900 // taller than any of these panes at the floor rate
    for (const paneH of [400, 480, 560]) {
      const { pxPerMin, colHeight } = timelineScale(windowMin, paneH)
      expect(pxPerMin).toBe(MIN_PX_PER_MIN)
      expect(colHeight).toBe(windowMin * MIN_PX_PER_MIN)
      expect(colHeight).toBeGreaterThan(paneH)
    }
  })

  it('grows a day shorter than the pane to fill it exactly', () => {
    const windowMin = MIN_WINDOW_MIN // taskWindow's own floor for a single short task
    const { colHeight } = timelineScale(windowMin, 560)
    expect(colHeight).toBeCloseTo(560, 0)
  })

  it('a shorter pane gets a genuinely smaller render, not the same pane-fitted size — proves growth is real, not self-cancelling', () => {
    const windowMin = 300 // natural = 300px, shorter than either pane below
    const a = timelineScale(windowMin, 500)
    const b = timelineScale(windowMin, 800)
    expect(a.colHeight).toBeCloseTo(500, 0)
    expect(b.colHeight).toBeCloseTo(800, 0)
    expect(b.colHeight).toBeGreaterThan(a.colHeight)
  })

  it('caps the grow-to-fill factor at MAX_PX_PER_MIN so a tiny window on a huge pane stays sane', () => {
    const { pxPerMin } = timelineScale(MIN_WINDOW_MIN, 100_000)
    expect(pxPerMin).toBe(MAX_PX_PER_MIN)
  })
})

describe('hourHasWork', () => {
  it('is true only for the exact clock hour real work falls in', () => {
    const laid = layoutDayTasks([task('T1', [{ start: '08:46', end: '08:59' }])])
    const win = { lo: 0, hi: 1440 }
    expect(hourHasWork(laid, win, 8)).toBe(true)
    expect(hourHasWork(laid, win, 7)).toBe(false)
    expect(hourHasWork(laid, win, 9)).toBe(false)
  })
})

describe('buildTimelineScale', () => {
  it('collapses a run of fully-idle whole hours to one hour worth of space, however many real hours it spans', () => {
    // Sittings at 12:27 AM and 8:46 AM — hours 1-7 have zero real work in them
    // (whole-hour granularity: no sub-minute sliver hidden inside), so that
    // 7-hour idle run must draw no taller than a single real active hour.
    const laid = layoutDayTasks([
      task('T1', [
        { start: '00:27', end: '00:59' },
        { start: '08:46', end: '08:59' },
      ]),
    ])
    const win = taskWindow(laid)
    const { pxPerMin, toPx } = buildTimelineScale(laid, win, 100_000) // huge pane: rules out grow-to-fill masking the cap
    const idleRunPx = toPx(7 * 60) - toPx(1 * 60) // start of hour 1 to start of hour 7
    const oneActiveHourPx = 60 * pxPerMin
    expect(idleRunPx).toBeLessThanOrEqual(oneActiveHourPx + 0.01)
  })

  it('never compresses an active hour — a real sitting always draws at the full honest rate', () => {
    const laid = layoutDayTasks([task('T1', [{ start: '08:00', end: '08:30' }])])
    const win = taskWindow(laid)
    const { pxPerMin, toPx } = buildTimelineScale(laid, win, 100_000)
    expect(toPx(8 * 60 + 30) - toPx(8 * 60)).toBeCloseTo(30 * pxPerMin, 5)
  })

  it('a day that is still tall after idle-hour collapsing scrolls rather than compressing further', () => {
    const laid = layoutDayTasks([
      task('T1', [{ start: '08:00', end: '10:00' }]),
      task('T2', [{ start: '10:00', end: '12:30' }]),
      task('T3', [{ start: '13:00', end: '15:00' }]),
      task('T4', [{ start: '15:00', end: '17:00' }]),
    ])
    const win = taskWindow(laid)
    for (const paneH of [400, 480, 560]) {
      const { colHeight } = buildTimelineScale(laid, win, paneH)
      expect(colHeight).toBeGreaterThan(paneH)
    }
  })

  it('a short day still grows to fill the pane', () => {
    const laid = layoutDayTasks([task('T1', [{ start: '08:00', end: '08:30' }])])
    const win = taskWindow(laid)
    const { colHeight } = buildTimelineScale(laid, win, 560)
    expect(colHeight).toBeCloseTo(560, 0)
  })
})
