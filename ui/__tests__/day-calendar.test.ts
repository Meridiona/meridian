//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, expect, it } from 'bun:test'
import { readFileSync } from 'fs'
import { addDays, addMonths, keyOf, monthGrid, parseKey } from '../components/timeline/DayCalendar'

describe('DayCalendar date math', () => {
  it('round-trips a key without drifting a day', () => {
    expect(keyOf(parseKey('2026-03-08'))).toBe('2026-03-08') // US DST start
    expect(keyOf(parseKey('2026-11-01'))).toBe('2026-11-01') // US DST end
  })

  it('adds days across month, year and DST boundaries', () => {
    expect(addDays('2026-03-01', -1)).toBe('2026-02-28')
    expect(addDays('2026-12-31', 1)).toBe('2027-01-01')
    expect(addDays('2026-03-07', 2)).toBe('2026-03-09')
  })

  it('clamps month moves to the target month length', () => {
    expect(addMonths('2026-03-31', -1)).toBe('2026-02-28')
    expect(addMonths('2028-03-31', -1)).toBe('2028-02-29')
    expect(addMonths('2026-01-15', -2)).toBe('2025-11-15')
  })

  it('always builds six Sunday-first weeks that contain the whole month', () => {
    for (const [y, m] of [[2026, 1], [2026, 9], [2027, 0]] as const) {
      const g = monthGrid(y, m)
      expect(g.length).toBe(42)
      expect(parseKey(g[0]).getDay()).toBe(0)
      expect(g).toContain(keyOf(new Date(y, m, 1, 12)))
      expect(g).toContain(keyOf(new Date(y, m + 1, 0, 12)))
    }
  })
})

describe('Toolbar wiring', () => {
  it('renders the calendar instead of a static date label', () => {
    const src = readFileSync(import.meta.dir + '/../components/timeline/Toolbar.tsx', 'utf8')
    expect(src).toContain('<DayCalendar')
    expect(src).toContain('onPickDay')
  })
})
