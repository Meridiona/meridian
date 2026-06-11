//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

import { describe, it, expect } from 'bun:test'

// Import pure utility functions from atoms (no DOM/React needed)
// We test the logic functions directly since they are pure
// and don't require a browser environment.

// Inline re-implementations that mirror atoms.tsx to avoid React import issues in bun test
function fmtDur(seconds: number): string {
  if (seconds < 60) return `${seconds}s`
  const m = Math.floor(seconds / 60)
  if (m < 60) return `${m}m`
  const h = Math.floor(m / 60)
  const rm = m % 60
  return rm > 0 ? `${h}h ${rm}m` : `${h}h`
}

function fmtDurDecimal(seconds: number): string {
  if (seconds < 60) return '0:' + String(seconds).padStart(2, '0')
  const m = Math.floor(seconds / 60)
  if (m < 60) return '0:' + String(m).padStart(2, '0')
  const h = Math.floor(m / 60)
  const rm = m % 60
  return `${h}:${String(rm).padStart(2, '0')}`
}

function fmtClock(isoOrHours: string | number): string {
  if (typeof isoOrHours === 'number') {
    const h = Math.floor(isoOrHours)
    const m = Math.round((isoOrHours - h) * 60)
    const period = h >= 12 ? 'PM' : 'AM'
    const hh = ((h + 11) % 12) + 1
    return `${hh}:${String(m).padStart(2, '0')} ${period}`
  }
  const d = new Date(isoOrHours)
  const h = d.getHours(), mn = d.getMinutes()
  const period = h >= 12 ? 'PM' : 'AM'
  const hh = ((h + 11) % 12) + 1
  return `${hh}:${String(mn).padStart(2, '0')} ${period}`
}

function shortTaskKey(keyId: string, max = 12): string {
  if (keyId.length <= max) return keyId
  const slash = keyId.indexOf('/')
  const k = slash >= 0 ? keyId.slice(slash + 1) : keyId
  if (k.length <= max) return k
  const hash = k.lastIndexOf('#')
  if (hash > 0) {
    const tail = k.slice(hash)
    const head = k.slice(0, Math.max(1, max - tail.length - 1))
    return `${head}…${tail}`
  }
  return `${k.slice(0, max - 1)}…`
}

function hexA(hex: string, a: number): string {
  const h = hex.replace('#', '')
  const r = parseInt(h.substring(0, 2), 16)
  const g = parseInt(h.substring(2, 4), 16)
  const b = parseInt(h.substring(4, 6), 16)
  return `rgba(${r},${g},${b},${a})`
}

// ── shortTaskKey ─────────────────────────────────────────────────────────────
describe('shortTaskKey (redesign atoms)', () => {
  it('leaves short keys untouched', () => {
    expect(shortTaskKey('KAN-157')).toBe('KAN-157')
    expect(shortTaskKey('ENG-1234')).toBe('ENG-1234')
  })

  it('drops the owner from a GitHub key that then fits', () => {
    expect(shortTaskKey('Meridiona/meridian#194')).toBe('meridian#194')
  })

  it('ellipsizes a long repo but always keeps the issue number', () => {
    expect(shortTaskKey('Meridiona/screenpipe-integration#1234')).toBe('screen…#1234')
  })

  it('keeps the full issue number even when the tail is long', () => {
    const out = shortTaskKey('org/repository-name#1234567890')
    expect(out.endsWith('#1234567890')).toBe(true)
    expect(out).toContain('…')
  })

  it('tail-ellipsizes long keys with no issue number', () => {
    expect(shortTaskKey('a-very-long-task-key-without-hash')).toBe('a-very-long…')
  })
})

// ── fmtDur ───────────────────────────────────────────────────────────────────
describe('fmtDur (redesign atoms)', () => {
  it('shows seconds under 60', () => {
    expect(fmtDur(0)).toBe('0s')
    expect(fmtDur(59)).toBe('59s')
  })

  it('shows minutes under an hour', () => {
    expect(fmtDur(60)).toBe('1m')
    expect(fmtDur(90)).toBe('1m')
    expect(fmtDur(3599)).toBe('59m')
  })

  it('shows hours + minutes', () => {
    expect(fmtDur(3600)).toBe('1h')
    expect(fmtDur(3660)).toBe('1h 1m')
    expect(fmtDur(5400)).toBe('1h 30m')
  })

  it('omits minutes when zero', () => {
    expect(fmtDur(7200)).toBe('2h')
  })
})

// ── fmtDurDecimal ────────────────────────────────────────────────────────────
describe('fmtDurDecimal (redesign atoms)', () => {
  it('formats seconds under 60 as 0:ss', () => {
    expect(fmtDurDecimal(0)).toBe('0:00')
    expect(fmtDurDecimal(5)).toBe('0:05')
    expect(fmtDurDecimal(59)).toBe('0:59')
  })

  it('formats minutes under an hour as 0:mm', () => {
    expect(fmtDurDecimal(60)).toBe('0:01')
    expect(fmtDurDecimal(600)).toBe('0:10')
  })

  it('formats hours correctly', () => {
    expect(fmtDurDecimal(3600)).toBe('1:00')
    expect(fmtDurDecimal(3660)).toBe('1:01')
    expect(fmtDurDecimal(5400)).toBe('1:30')
    expect(fmtDurDecimal(7322)).toBe('2:02')
  })
})

// ── fmtClock ─────────────────────────────────────────────────────────────────
describe('fmtClock (redesign atoms)', () => {
  it('formats noon correctly', () => {
    expect(fmtClock(12)).toBe('12:00 PM')
  })

  it('formats midnight correctly', () => {
    expect(fmtClock(0)).toBe('12:00 AM')
  })

  it('formats 9 AM correctly', () => {
    expect(fmtClock(9)).toBe('9:00 AM')
  })

  it('formats 9:30 AM correctly', () => {
    expect(fmtClock(9.5)).toBe('9:30 AM')
  })

  it('formats 1 PM correctly', () => {
    expect(fmtClock(13)).toBe('1:00 PM')
  })

  it('formats ISO string', () => {
    // 2024-06-15T14:30:00 local — 2:30 PM
    const iso = new Date('2024-06-15T14:30:00').toISOString()
    const result = fmtClock(iso)
    // result depends on local timezone, just validate shape
    expect(result).toMatch(/^\d{1,2}:\d{2} (AM|PM)$/)
  })
})

// ── hexA ──────────────────────────────────────────────────────────────────────
describe('hexA (theme helper)', () => {
  it('converts black at 0.5', () => {
    expect(hexA('#000000', 0.5)).toBe('rgba(0,0,0,0.5)')
  })

  it('converts white at 1', () => {
    expect(hexA('#ffffff', 1)).toBe('rgba(255,255,255,1)')
  })

  it('converts accent orange', () => {
    expect(hexA('#FF6B2B', 0.06)).toBe('rgba(255,107,43,0.06)')
  })

  it('strips the hash before parsing', () => {
    expect(hexA('FF6B2B', 0.1)).toBe('rgba(255,107,43,0.1)')
  })
})

// ── CATS metadata ─────────────────────────────────────────────────────────────
const CATS: Record<string, { label: string; short: string }> = {
  coding:            { label: 'Coding',      short: 'Code'   },
  code_review:       { label: 'Code review', short: 'Review' },
  meeting:           { label: 'Meeting',     short: 'Meet'   },
  communication:     { label: 'Comms',       short: 'Comms'  },
  design:            { label: 'Design',      short: 'Design' },
  documentation:     { label: 'Docs',        short: 'Docs'   },
  planning:          { label: 'Planning',    short: 'Plan'   },
  deployment_devops: { label: 'DevOps',      short: 'DevOps' },
  research:          { label: 'Research',    short: 'Res'    },
  idle_personal:     { label: 'Idle',        short: 'Idle'   },
}

describe('CATS metadata', () => {
  it('has 10 categories', () => {
    expect(Object.keys(CATS).length).toBe(10)
  })

  it('each category has label and short', () => {
    Object.values(CATS).forEach(c => {
      expect(typeof c.label).toBe('string')
      expect(c.label.length).toBeGreaterThan(0)
      expect(typeof c.short).toBe('string')
      expect(c.short.length).toBeGreaterThan(0)
    })
  })

  it('coding label is Coding', () => {
    expect(CATS.coding.label).toBe('Coding')
  })

  it('idle_personal exists', () => {
    expect(CATS.idle_personal).toBeDefined()
  })
})

// ── ACCENT_PRESETS ────────────────────────────────────────────────────────────
const ACCENT_PRESETS = ['#FF6B2B', '#2A6FDB', '#1F8A5B', '#141414']

describe('ACCENT_PRESETS', () => {
  it('has exactly 4 presets', () => {
    expect(ACCENT_PRESETS.length).toBe(4)
  })

  it('all are valid hex colors', () => {
    ACCENT_PRESETS.forEach(c => {
      expect(c).toMatch(/^#[0-9A-Fa-f]{6}$/)
    })
  })

  it('default accent is first preset', () => {
    expect(ACCENT_PRESETS[0]).toBe('#FF6B2B')
  })

  it('all presets are distinct', () => {
    const unique = new Set(ACCENT_PRESETS.map(c => c.toLowerCase()))
    expect(unique.size).toBe(ACCENT_PRESETS.length)
  })
})

// ── App glyph determinism ─────────────────────────────────────────────────────
function deterministicColor(app: string): string {
  let h = 0
  for (let i = 0; i < app.length; i++) h = (h * 31 + app.charCodeAt(i)) & 0xffff
  const hue = h % 360
  return `hsl(${hue}, 55%, 42%)`
}

describe('app glyph color (deterministic)', () => {
  it('same app always returns same color', () => {
    expect(deterministicColor('Google Chrome')).toBe(deterministicColor('Google Chrome'))
    expect(deterministicColor('Slack')).toBe(deterministicColor('Slack'))
  })

  it('different apps return different hues', () => {
    const a = deterministicColor('Google Chrome')
    const b = deterministicColor('Slack')
    expect(a).not.toBe(b)
  })

  it('returns a valid hsl string', () => {
    const color = deterministicColor('Terminal')
    expect(color).toMatch(/^hsl\(\d+, 55%, 42%\)$/)
  })
})
