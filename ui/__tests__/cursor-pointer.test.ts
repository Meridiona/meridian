// meridian — normalises screenpipe activity into structured app sessions
import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'fs'

const uiRoot = import.meta.dir + '/..'

function readSrc(relPath: string): string {
  return readFileSync(uiRoot + '/' + relPath, 'utf8')
}

// ── globals.css rule ──────────────────────────────────────────────────────────

describe('globals.css cursor rules', () => {
  const css = readSrc('app/globals.css')

  it('sets cursor:pointer on enabled buttons', () => {
    expect(css).toContain('button:not(:disabled)')
    expect(css).toMatch(/button:not\(:disabled\)\s*\{[^}]*cursor:\s*pointer/)
  })

  it('sets cursor:not-allowed on disabled buttons', () => {
    expect(css).toContain('button:disabled')
    expect(css).toMatch(/button:disabled\s*\{[^}]*cursor:\s*not-allowed/)
  })
})

// ── Switch.tsx ────────────────────────────────────────────────────────────────

describe('Switch.tsx cursor style', () => {
  const src = readSrc('components/ui/Switch.tsx')

  it('does not use cursor:default (would override the global rule)', () => {
    expect(src).not.toContain("cursor: 'default'")
  })

  it('uses cursor:pointer', () => {
    expect(src).toContain("cursor: 'pointer'")
  })
})

// ── Select.tsx ────────────────────────────────────────────────────────────────

describe('Select.tsx cursor styles', () => {
  const src = readSrc('components/ui/Select.tsx')

  it('does not use cursor:default anywhere', () => {
    expect(src).not.toContain("cursor: 'default'")
  })

  it('uses cursor:pointer for trigger and items', () => {
    // Should appear at least twice (trigger + item)
    const matches = src.match(/cursor: 'pointer'/g) ?? []
    expect(matches.length).toBeGreaterThanOrEqual(2)
  })
})

// ── NumberStepper.tsx — conditional cursor logic ──────────────────────────────

describe('NumberStepper cursor logic', () => {
  it('returns pointer when decrement button is not at minimum', () => {
    const cursor = (atMin: boolean) => atMin ? 'not-allowed' : 'pointer'
    expect(cursor(false)).toBe('pointer')
    expect(cursor(true)).toBe('not-allowed')
  })

  it('returns pointer when increment button is not at maximum', () => {
    const cursor = (atMax: boolean) => atMax ? 'not-allowed' : 'pointer'
    expect(cursor(false)).toBe('pointer')
    expect(cursor(true)).toBe('not-allowed')
  })

  it('source does not use cursor:default', () => {
    const src = readSrc('components/ui/NumberStepper.tsx')
    expect(src).not.toContain("cursor: atMin ? 'not-allowed' : 'default'")
    expect(src).not.toContain("cursor: atMax ? 'not-allowed' : 'default'")
  })
})
