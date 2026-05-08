import { describe, it, expect } from 'bun:test'
import { formatDuration } from '../lib/format'
import { localDayBounds } from '../lib/date-utils'
import { getAppColor, getAppColorBg, getAppInitial } from '../lib/app-colors'

// ---------------------------------------------------------------------------
// formatDuration
// ---------------------------------------------------------------------------

describe('formatDuration', () => {
  it('returns "0s" for 0 seconds', () => {
    expect(formatDuration(0)).toBe('0s')
  })

  it('returns "45s" for 45 seconds', () => {
    expect(formatDuration(45)).toBe('45s')
  })

  it('returns "1m" for exactly 60 seconds (no trailing 0s)', () => {
    // When s === 0, the function omits the seconds segment → "1m"
    expect(formatDuration(60)).toBe('1m')
  })

  it('returns "1m 47s" for 107 seconds', () => {
    expect(formatDuration(107)).toBe('1m 47s')
  })

  it('returns "1h 0m" for exactly 3600 seconds', () => {
    expect(formatDuration(3600)).toBe('1h 0m')
  })

  it('returns "1h 15m" for 4500 seconds', () => {
    expect(formatDuration(4500)).toBe('1h 15m')
  })
})

// ---------------------------------------------------------------------------
// localDayBounds
// ---------------------------------------------------------------------------

describe('localDayBounds', () => {
  it('start parses to midnight local time', () => {
    const { start } = localDayBounds('2025-03-15')
    const d = new Date(start)
    // Reconstruct what local midnight looks like
    const localMidnight = new Date('2025-03-15T00:00:00')
    expect(d.getTime()).toBe(localMidnight.getTime())
  })

  it('end parses to 23:59:59.999 local time', () => {
    const { end } = localDayBounds('2025-03-15')
    const d = new Date(end)
    const localEndOfDay = new Date('2025-03-15T23:59:59.999')
    expect(d.getTime()).toBe(localEndOfDay.getTime())
  })

  it('end is always after start', () => {
    const { start, end } = localDayBounds('2025-03-15')
    expect(new Date(end).getTime()).toBeGreaterThan(new Date(start).getTime())
  })

  it('returns ISO strings', () => {
    const { start, end } = localDayBounds('2024-01-01')
    expect(typeof start).toBe('string')
    expect(typeof end).toBe('string')
    // ISO strings can be parsed back into valid dates
    expect(isNaN(new Date(start).getTime())).toBe(false)
    expect(isNaN(new Date(end).getTime())).toBe(false)
  })
})

// ---------------------------------------------------------------------------
// getAppColor
// ---------------------------------------------------------------------------

describe('getAppColor', () => {
  it('returns the muted grey for "(idle)"', () => {
    expect(getAppColor('(idle)')).toBe('#C8C6C1')
  })

  it('returns the muted grey for "(away)"', () => {
    expect(getAppColor('(away)')).toBe('#C8C6C1')
  })

  it('is deterministic — same app name always returns the same color', () => {
    expect(getAppColor('Google Chrome')).toBe(getAppColor('Google Chrome'))
    expect(getAppColor('Slack')).toBe(getAppColor('Slack'))
    expect(getAppColor('Terminal')).toBe(getAppColor('Terminal'))
  })

  it('returns an hsl() string for regular app names', () => {
    const color = getAppColor('Google Chrome')
    expect(color).toMatch(/^hsl\(\d+, \d+%, \d+%\)$/)
  })

  it('different apps return different colors (5 common names)', () => {
    const names = ['Google Chrome', 'Safari', 'Terminal', 'VS Code', 'Finder']
    const colors = names.map(getAppColor)
    const uniqueColors = new Set(colors)
    expect(uniqueColors.size).toBe(names.length)
  })
})

// ---------------------------------------------------------------------------
// getAppColorBg
// ---------------------------------------------------------------------------

describe('getAppColorBg', () => {
  it('returns the light muted bg for "(idle)"', () => {
    expect(getAppColorBg('(idle)')).toBe('#EDEBE6')
  })

  it('returns the light muted bg for "(away)"', () => {
    expect(getAppColorBg('(away)')).toBe('#EDEBE6')
  })

  it('returns an hsl() string for regular apps', () => {
    const bg = getAppColorBg('Slack')
    expect(bg).toMatch(/^hsl\(\d+, \d+%, \d+%\)$/)
  })
})

// ---------------------------------------------------------------------------
// getAppInitial
// ---------------------------------------------------------------------------

describe('getAppInitial', () => {
  it('returns "G" for "Google Chrome"', () => {
    expect(getAppInitial('Google Chrome')).toBe('G')
  })

  it('returns uppercase "T" for "terminal"', () => {
    expect(getAppInitial('terminal')).toBe('T')
  })

  it('returns empty string for empty input', () => {
    // charAt(0) on an empty string returns "" — no "?" fallback in source
    expect(getAppInitial('')).toBe('')
  })

  it('returns an em-dash for "(idle)"', () => {
    expect(getAppInitial('(idle)')).toBe('—')
  })

  it('returns an em-dash for "(away)"', () => {
    expect(getAppInitial('(away)')).toBe('—')
  })

  it('trims leading whitespace before taking the initial', () => {
    expect(getAppInitial('  Safari')).toBe('S')
  })
})
