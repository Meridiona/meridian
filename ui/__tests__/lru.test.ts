//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, it, expect } from 'bun:test'
import { LruMap } from '../lib/lru'

// LruMap backs the module-level webview stores (planStore.ts, useWorklog.ts,
// app-icons.ts) that live for the whole Tauri session — this is the shared
// eviction logic added so those stores stop growing by one entry per
// distinct key forever (memory-leak-audit fix).

describe('LruMap', () => {
  it('get/set round-trip like a Map for keys within capacity', () => {
    const m = new LruMap<string, number>(3)
    m.set('a', 1)
    m.set('b', 2)
    expect(m.get('a')).toBe(1)
    expect(m.get('b')).toBe(2)
    expect(m.get('missing')).toBeUndefined()
    expect(m.size).toBe(2)
  })

  it('stores a value of null distinctly from "not cached" (undefined)', () => {
    const m = new LruMap<string, string | null>(3)
    m.set('a', null)
    expect(m.has('a')).toBe(true)
    expect(m.get('a')).toBeNull()
    expect(m.get('never-set')).toBeUndefined()
  })

  it('evicts the least-recently-used entry once capacity is exceeded', () => {
    const m = new LruMap<string, number>(2)
    m.set('a', 1)
    m.set('b', 2)
    m.set('c', 3) // capacity 2 — 'a' (oldest, untouched) must be evicted

    expect(m.has('a')).toBe(false)
    expect(m.get('b')).toBe(2)
    expect(m.get('c')).toBe(3)
    expect(m.size).toBe(2)
  })

  it('a get() on a key protects it from eviction (recency, not insertion order)', () => {
    const m = new LruMap<string, number>(2)
    m.set('a', 1)
    m.set('b', 2)
    m.get('a') // touch 'a' — now 'b' is the least-recently-used
    m.set('c', 3)

    expect(m.has('b')).toBe(false)
    expect(m.get('a')).toBe(1)
    expect(m.get('c')).toBe(3)
  })

  it('overwriting an existing key bumps its recency without growing size', () => {
    const m = new LruMap<string, number>(2)
    m.set('a', 1)
    m.set('b', 2)
    m.set('a', 10) // re-set 'a' — now 'b' is the least-recently-used
    m.set('c', 3)

    expect(m.size).toBe(2)
    expect(m.has('b')).toBe(false)
    expect(m.get('a')).toBe(10)
  })

  it('never exceeds capacity across many inserts', () => {
    const m = new LruMap<number, number>(5)
    for (let i = 0; i < 1000; i++) {
      m.set(i, i)
    }
    expect(m.size).toBe(5)
    // Only the last 5 keys (995..999) should survive.
    for (let i = 995; i < 1000; i++) {
      expect(m.get(i)).toBe(i)
    }
    expect(m.get(0)).toBeUndefined()
  })

  it('delete removes an entry and reduces size', () => {
    const m = new LruMap<string, number>(3)
    m.set('a', 1)
    expect(m.delete('a')).toBe(true)
    expect(m.has('a')).toBe(false)
    expect(m.size).toBe(0)
  })

  it('rejects a non-positive capacity', () => {
    expect(() => new LruMap<string, number>(0)).toThrow()
    expect(() => new LruMap<string, number>(-1)).toThrow()
  })

  it('rejects NaN/Infinity/fractional capacity (they would disable eviction)', () => {
    // `NaN < 1` and `Infinity < 1` are both false, so a bare `< 1` guard would
    // accept them and then `size > capacity` never fires — no bound at all.
    expect(() => new LruMap<string, number>(NaN)).toThrow()
    expect(() => new LruMap<string, number>(Infinity)).toThrow()
    expect(() => new LruMap<string, number>(2.5)).toThrow()
  })

  it('evicts an explicit undefined key like any other (no sentinel confusion)', () => {
    // An `undefined` key is a valid Map entry; eviction must not skip it just
    // because it equals the empty-iterator sentinel value.
    const m = new LruMap<string | undefined, number>(2)
    m.set(undefined, 1) // oldest
    m.set('b', 2)
    m.set('c', 3) // pushes past capacity — the undefined-keyed entry must go
    expect(m.has(undefined)).toBe(false)
    expect(m.get('b')).toBe(2)
    expect(m.get('c')).toBe(3)
    expect(m.size).toBe(2)
  })
})
