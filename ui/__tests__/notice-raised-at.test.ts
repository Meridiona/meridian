//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The db.corrupt notice survived its own repair twice on 2026-08-04: the
// banner showed a 05:15 fault over a healthy database twelve hours later, and
// nothing on screen distinguished it from a live alarm. The repair now scrubs
// the notice (src/db/repair/rebuild.rs), and this stamp is the belt to that
// suspender: a banner that says WHEN it was raised lets a user (and support)
// spot a stale one at a glance.

import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'fs'
import { join } from 'path'
import { formatSince } from '../lib/notice-time'

describe('formatSince', () => {
  // Local-time constructors round-tripped through toISOString keep the
  // instant, so the local-day comparison below is timezone-independent.
  const now = new Date(2026, 7, 4, 17, 30)

  it('same-day notice shows the time alone', () => {
    const out = formatSince(new Date(2026, 7, 4, 10, 45).toISOString(), now)
    expect(out.startsWith('since ')).toBe(true)
    expect(out).toContain('45')
    expect(out).not.toContain('Aug')
  })

  it('an older notice carries the date too', () => {
    const sameDay = formatSince(new Date(2026, 7, 4, 10, 45).toISOString(), now)
    const older = formatSince(new Date(2026, 7, 1, 9, 5).toISOString(), now)
    expect(older.startsWith('since ')).toBe(true)
    expect(older).toContain('05')
    // The date must actually be visible - that is the entire point.
    expect(older.length).toBeGreaterThan(sameDay.length)
  })

  it('an unparseable timestamp renders nothing rather than "Invalid Date"', () => {
    expect(formatSince('not-a-time', now)).toBe('')
    expect(formatSince('', now)).toBe('')
  })

  it('is user-facing app text: plain hyphens only', () => {
    const src = readFileSync(join(import.meta.dir, '../lib/notice-time.ts'), 'utf8')
    expect(src).not.toMatch(/[–—]/)
  })
})

describe('NoticeBar renders the stamp', () => {
  it('imports formatSince and applies it to raised_at', () => {
    const src = readFileSync(join(import.meta.dir, '../components/NoticeBar.tsx'), 'utf8')
    expect(src).toMatch(/from '@\/lib\/notice-time'/)
    expect(src).toContain('formatSince(n.raised_at')
  })
})
