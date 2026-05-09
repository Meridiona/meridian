// meridian — AI activity intelligence by Meridiona

import { describe, it, expect } from 'bun:test'
import { getCategoryMeta, CATEGORY_META } from '../lib/category-colors'
import type { Category } from '../lib/category-colors'

// ---------------------------------------------------------------------------
// CATEGORY_META — completeness
// ---------------------------------------------------------------------------

const ALL_CATEGORIES: Category[] = [
  'coding', 'code_review', 'meeting', 'communication',
  'design', 'documentation', 'planning', 'deployment_devops',
  'research', 'idle_personal',
]

describe('CATEGORY_META', () => {
  it('defines exactly 10 categories', () => {
    expect(Object.keys(CATEGORY_META).length).toBe(10)
  })

  it('every category has a non-empty label', () => {
    for (const cat of ALL_CATEGORIES) {
      expect(CATEGORY_META[cat].label.length).toBeGreaterThan(0)
    }
  })

  it('every category has a hex color', () => {
    for (const cat of ALL_CATEGORIES) {
      expect(CATEGORY_META[cat].color).toMatch(/^#[0-9A-Fa-f]{6}$/)
    }
  })

  it('every category has a hex bg', () => {
    for (const cat of ALL_CATEGORIES) {
      expect(CATEGORY_META[cat].bg).toMatch(/^#[0-9A-Fa-f]{6}$/)
    }
  })

  it('every category has an emoji', () => {
    for (const cat of ALL_CATEGORIES) {
      expect(CATEGORY_META[cat].emoji.length).toBeGreaterThan(0)
    }
  })

  it('all colors are distinct', () => {
    const colors = ALL_CATEGORIES.map(c => CATEGORY_META[c].color)
    expect(new Set(colors).size).toBe(ALL_CATEGORIES.length)
  })

  it('all bg colors are distinct', () => {
    const bgs = ALL_CATEGORIES.map(c => CATEGORY_META[c].bg)
    expect(new Set(bgs).size).toBe(ALL_CATEGORIES.length)
  })

  it('coding is blue (#4F7BE8)', () => {
    expect(CATEGORY_META['coding'].color).toBe('#4F7BE8')
  })

  it('deployment_devops is red (#EF4444)', () => {
    expect(CATEGORY_META['deployment_devops'].color).toBe('#EF4444')
  })

  it('idle_personal is gray (#9CA3AF)', () => {
    expect(CATEGORY_META['idle_personal'].color).toBe('#9CA3AF')
  })
})

// ---------------------------------------------------------------------------
// getCategoryMeta
// ---------------------------------------------------------------------------

describe('getCategoryMeta', () => {
  it('returns the correct meta for a known category', () => {
    const meta = getCategoryMeta('coding')
    expect(meta.label).toBe('Coding')
    expect(meta.color).toBe('#4F7BE8')
  })

  it('falls back to idle_personal for unknown strings', () => {
    const meta = getCategoryMeta('totally_unknown_category')
    expect(meta).toEqual(CATEGORY_META['idle_personal'])
  })

  it('falls back to idle_personal for empty string', () => {
    const meta = getCategoryMeta('')
    expect(meta).toEqual(CATEGORY_META['idle_personal'])
  })

  it('is stable — same input always returns same result', () => {
    expect(getCategoryMeta('research')).toEqual(getCategoryMeta('research'))
  })

  it('returns correct meta for all 10 known categories', () => {
    for (const cat of ALL_CATEGORIES) {
      const meta = getCategoryMeta(cat)
      expect(meta).toEqual(CATEGORY_META[cat])
    }
  })
})
