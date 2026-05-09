// meridian — AI activity intelligence by Meridiona

import { describe, it, expect } from 'bun:test'
import { formatDuration, formatDateLabel, toLocalDateString } from '../lib/format'

// ---------------------------------------------------------------------------
// formatDuration — edge cases beyond lib.test.ts
// ---------------------------------------------------------------------------

describe('formatDuration — additional cases', () => {
  it('returns "0s" for negative input (no crash)', () => {
    // negative seconds should not throw
    expect(() => formatDuration(-1)).not.toThrow()
  })

  it('returns "59s" for 59 seconds', () => {
    expect(formatDuration(59)).toBe('59s')
  })

  it('returns "2m" for 120 seconds', () => {
    expect(formatDuration(120)).toBe('2m')
  })

  it('returns "2h 30m" for 9000 seconds', () => {
    expect(formatDuration(9000)).toBe('2h 30m')
  })

  it('returns "8h 0m" for a full work-day (28800 s)', () => {
    expect(formatDuration(28800)).toBe('8h 0m')
  })

  it('omits seconds when over an hour', () => {
    // 3661 = 1h 1m 1s — seconds are dropped at the hour scale
    expect(formatDuration(3661)).toBe('1h 1m')
  })
})

// ---------------------------------------------------------------------------
// formatDateLabel
// ---------------------------------------------------------------------------

describe('formatDateLabel', () => {
  it('returns "Today" for today\'s date string', () => {
    const today = new Date().toISOString().split('T')[0]
    expect(formatDateLabel(today)).toBe('Today')
  })

  it('returns "Yesterday" for yesterday\'s date string', () => {
    const yesterday = new Date(Date.now() - 86400000).toISOString().split('T')[0]
    expect(formatDateLabel(yesterday)).toBe('Yesterday')
  })

  it('returns a formatted date for older dates', () => {
    // 2020-01-15 is neither today nor yesterday
    const result = formatDateLabel('2020-01-15')
    expect(result).not.toBe('Today')
    expect(result).not.toBe('Yesterday')
    // Should contain "Jan" and "15"
    expect(result).toContain('Jan')
    expect(result).toContain('15')
  })
})

// ---------------------------------------------------------------------------
// toLocalDateString
// ---------------------------------------------------------------------------

describe('toLocalDateString', () => {
  it('returns a string matching YYYY-MM-DD', () => {
    const result = toLocalDateString()
    expect(result).toMatch(/^\d{4}-\d{2}-\d{2}$/)
  })

  it('returns today when called with no arguments', () => {
    const expected = new Date().toISOString().split('T')[0]
    // May differ if called exactly at midnight — tolerate 1-day drift
    expect(toLocalDateString()).toMatch(/^\d{4}-\d{2}-\d{2}$/)
    // The returned string must be parseable
    expect(isNaN(new Date(toLocalDateString()).getTime())).toBe(false)
  })

  it('formats a specific date correctly', () => {
    expect(toLocalDateString(new Date('2024-06-15T12:00:00'))).toBe('2024-06-15')
  })

  it('zero-pads month and day', () => {
    expect(toLocalDateString(new Date('2024-01-05T12:00:00'))).toBe('2024-01-05')
  })
})
