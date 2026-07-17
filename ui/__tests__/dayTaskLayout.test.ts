//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, it, expect } from 'bun:test'
import {
  hhmmToMin,
  clockLabel,
  taskWindow,
  taskRangeLabel,
  layoutDayTasks,
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
