//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, it, expect } from 'bun:test'
import { isSnoozed } from '../components/timeline/useActionItems'

describe('isSnoozed', () => {
  it('hides a kind present in the snooze map', () => {
    expect(isSnoozed('drafts', { drafts: '2026-01-01T04:00:00Z' })).toBe(true)
  })

  it('shows a kind absent from the snooze map', () => {
    expect(isSnoozed('drafts', { mustfix: '2026-01-01T00:00:00Z' })).toBe(false)
  })

  it('shows every kind when the map is empty', () => {
    expect(isSnoozed('mustfix', {})).toBe(false)
    expect(isSnoozed('cleanup', {})).toBe(false)
    expect(isSnoozed('drafts', {})).toBe(false)
  })

  it('checks each kind independently — one snooze does not hide the others', () => {
    const snoozes = { cleanup: '2026-06-01T00:00:00Z' }
    expect(isSnoozed('cleanup', snoozes)).toBe(true)
    expect(isSnoozed('mustfix', snoozes)).toBe(false)
    expect(isSnoozed('drafts', snoozes)).toBe(false)
  })
})
