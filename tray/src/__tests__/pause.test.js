//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use strict'

const { fmtCountdown, parsePauseMins, pauseLabel } = require('../pause-utils.js')

// ── US-1: Quick preset pause ─────────────────────────────────────────────────
// The preset buttons pass a fixed `data-secs` value straight to
// pause_for_duration. The JS layer doesn't validate — just forward. These tests
// verify the module contract is satisfied for each preset.

describe('parsePauseMins — preset equivalents', () => {
  test('5-minute preset rounds to 5 whole minutes', () => {
    // 300 s / 60 = 5 min; parsePauseMins("5") must return 5
    expect(parsePauseMins('5')).toBe(5)
  })
  test('15-minute preset', () => {
    expect(parsePauseMins('15')).toBe(15)
  })
  test('30-minute preset', () => {
    expect(parsePauseMins('30')).toBe(30)
  })
  test('60-minute preset (1 hr)', () => {
    expect(parsePauseMins('60')).toBe(60)
  })
})

// ── US-2: Custom duration entry ──────────────────────────────────────────────
// The "···" button shows the custom input. Clicking Pause validates the field
// value via parsePauseMins.

describe('parsePauseMins — valid custom durations', () => {
  test('returns integer minutes for positive numeric strings', () => {
    expect(parsePauseMins('1')).toBe(1)
    expect(parsePauseMins('45')).toBe(45)
    expect(parsePauseMins('90')).toBe(90)
  })
  test('480 min (8 h) is the upper boundary — accepted', () => {
    expect(parsePauseMins('480')).toBe(480)
  })
  test('truncates decimal fractions (input type=number yields integer strings)', () => {
    expect(parsePauseMins('5.7')).toBe(5)
  })
  test('leading zeros parse correctly', () => {
    expect(parsePauseMins('005')).toBe(5)
  })
  test('whitespace-padded values parse correctly (browser trims for number input)', () => {
    expect(parsePauseMins('  10  ')).toBe(10)
  })
  test('scientific-notation string parses as integer prefix (parseInt stops at "e")', () => {
    // '1e2' → parseInt stops at 'e' → 1, which is within [1, 480] → valid
    expect(parsePauseMins('1e2')).toBe(1)
  })
})

describe('parsePauseMins — invalid custom durations (silent rejection)', () => {
  // When invalid, the confirm handler does an early-return without invoking pause.
  test('empty string → null', () => {
    expect(parsePauseMins('')).toBeNull()
  })
  test('zero → null (0 is reserved for resume-now in pause_for_duration)', () => {
    expect(parsePauseMins('0')).toBeNull()
  })
  test('negative number → null', () => {
    expect(parsePauseMins('-5')).toBeNull()
  })
  test('non-numeric text → null', () => {
    expect(parsePauseMins('abc')).toBeNull()
    expect(parsePauseMins('mins')).toBeNull()
  })
  test('whitespace-only → null', () => {
    expect(parsePauseMins('   ')).toBeNull()
  })
  test('above 8-hour cap → null (bypassed HTML max="480" attribute)', () => {
    expect(parsePauseMins('481')).toBeNull()
    expect(parsePauseMins('9999')).toBeNull()
  })
})

// ── US-3: Cancel custom entry ────────────────────────────────────────────────
// Clicking Cancel hides the custom input and shows the picker. Covered by
// DOM-level tests in the integration suite; at the unit level, parsePauseMins
// has no side-effects to cancel — the Cancel button simply reverts visibility.

// ── US-4: Countdown display while paused ────────────────────────────────────

describe('fmtCountdown — remaining time display', () => {
  test('exactly zero → "0:00"', () => {
    expect(fmtCountdown(0)).toBe('0:00')
  })
  test('negative (already expired) → "0:00"', () => {
    expect(fmtCountdown(-5000)).toBe('0:00')
  })
  test('fractional milliseconds round up (Math.ceil)', () => {
    expect(fmtCountdown(1)).toBe('0:01')    // ceil(0.001s) = 1
    expect(fmtCountdown(1001)).toBe('0:02') // ceil(1.001s) = 2
  })
  test('whole-second boundary (no rounding)', () => {
    expect(fmtCountdown(1000)).toBe('0:01') // ceil(1.0s) = 1, no rounding fires
  })
  test('under a minute', () => {
    expect(fmtCountdown(30_000)).toBe('0:30')
    expect(fmtCountdown(59_000)).toBe('0:59')
  })
  test('exactly one minute', () => {
    expect(fmtCountdown(60_000)).toBe('1:00')
  })
  test('5 minutes', () => {
    expect(fmtCountdown(300_000)).toBe('5:00')
  })
  test('5 min 30 s', () => {
    expect(fmtCountdown(330_000)).toBe('5:30')
  })
  test('seconds portion is always two digits', () => {
    expect(fmtCountdown(61_000)).toBe('1:01')
    expect(fmtCountdown(605_000)).toBe('10:05')
  })
  test('multi-hour durations display total minutes', () => {
    // 90 min = 5400 s
    expect(fmtCountdown(5_400_000)).toBe('90:00')
  })
})

// ── US-5: Toast notification label (pause_for_duration → sys::notify) ───────
// The Rust daemon builds a label from the requested seconds. pauseLabel()
// mirrors that logic for JS-side testing.

describe('pauseLabel — notification wording', () => {
  test('sub-minute pause uses seconds', () => {
    expect(pauseLabel(1)).toBe('1 second')
    expect(pauseLabel(30)).toBe('30 seconds')
    expect(pauseLabel(59)).toBe('59 seconds')
  })
  test('minute-boundary', () => {
    expect(pauseLabel(60)).toBe('1 minute')
  })
  test('plural minutes', () => {
    expect(pauseLabel(120)).toBe('2 minutes')
    expect(pauseLabel(1500)).toBe('25 minutes')
    expect(pauseLabel(3540)).toBe('59 minutes')
  })
  test('exactly one hour', () => {
    expect(pauseLabel(3600)).toBe('1 hour')
  })
  test('plural hours (fractional hours truncated to whole hours)', () => {
    expect(pauseLabel(7200)).toBe('2 hours')
    expect(pauseLabel(28800)).toBe('8 hours') // 8 h max custom
  })
  test('1h 30m rounds down to 1 hour', () => {
    // The label shows whole hours only (matches Rust impl: mins / 60)
    expect(pauseLabel(5400)).toBe('1 hour')
  })
})

// ── US-6: Resume-now (seconds = 0) ──────────────────────────────────────────
// pause_for_duration(0) is the resume path; pauseLabel should not be reached
// for the 0-second case in the Rust side. Verify the JS helper doesn't crash:

describe('pauseLabel — edge: zero seconds', () => {
  test('0 seconds → "0 seconds" (resume path bypasses label in Rust)', () => {
    expect(pauseLabel(0)).toBe('0 seconds')
  })
})
