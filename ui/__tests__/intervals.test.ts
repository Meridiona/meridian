//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, it, expect } from 'bun:test'
import { unionSeconds, countSwitches, mergeIntervals, intersectSeconds, clampIntervals } from '../lib/intervals'

// ISO helper: minutes-past-midnight UTC → ISO string, keeps cases readable.
const at = (min: number) => new Date(Date.UTC(2026, 0, 1, 0, min, 0)).toISOString()
const iv = (startMin: number, endMin: number) => ({ started_at: at(startMin), ended_at: at(endMin) })

// ---------------------------------------------------------------------------
// unionSeconds — the anti-double-count core
// ---------------------------------------------------------------------------

describe('unionSeconds', () => {
  it('returns 0 for no intervals', () => {
    expect(unionSeconds([])).toBe(0)
  })

  it('returns the span of a single interval', () => {
    expect(unionSeconds([iv(0, 10)])).toBe(600) // 10 min
  })

  it('sums two disjoint intervals', () => {
    expect(unionSeconds([iv(0, 10), iv(20, 25)])).toBe(900) // 10 + 5 min
  })

  it('counts overlapping intervals once (the double-count bug)', () => {
    // foreground 0–30, coding-agent overlay 10–40 → union is 0–40 = 40 min,
    // NOT 30 + 30 = 60 min.
    expect(unionSeconds([iv(0, 30), iv(10, 40)])).toBe(2400)
  })

  it('counts a fully nested interval once', () => {
    expect(unionSeconds([iv(0, 60), iv(20, 30)])).toBe(3600)
  })

  it('merges touching intervals without gaps or overlap', () => {
    expect(unionSeconds([iv(0, 10), iv(10, 20)])).toBe(1200)
  })

  it('is order-independent', () => {
    expect(unionSeconds([iv(20, 40), iv(0, 30), iv(35, 50)])).toBe(unionSeconds([iv(0, 30), iv(20, 40), iv(35, 50)]))
  })

  it('discards corrupt intervals where end precedes start', () => {
    // a segmentation bug (ended_at < started_at) must not subtract time.
    expect(unionSeconds([iv(0, 10), { started_at: at(30), ended_at: at(5) }])).toBe(600)
  })

  it('discards zero-length and unparseable intervals', () => {
    expect(unionSeconds([iv(5, 5), { started_at: 'nope', ended_at: 'nope' }, iv(0, 10)])).toBe(600)
  })
})

// ---------------------------------------------------------------------------
// countSwitches — real context switches, not capture jitter
// ---------------------------------------------------------------------------

const s = (app: string, startMin: number, dur: number) => ({ app, started_at: at(startMin), dur })

describe('countSwitches', () => {
  it('returns 0 for an empty stream', () => {
    expect(countSwitches([], 15)).toBe(0)
  })

  it('returns 0 for a single session', () => {
    expect(countSwitches([s('Code', 0, 600)], 15)).toBe(0)
  })

  it('does not count consecutive sessions of the same app', () => {
    expect(countSwitches([s('Code', 0, 600), s('Code', 10, 600)], 15)).toBe(2 - 2) // 0
  })

  it('counts each app change between consecutive sessions', () => {
    expect(countSwitches([s('Code', 0, 600), s('Chrome', 10, 600), s('Code', 20, 600)], 15)).toBe(2)
  })

  it('ignores sub-floor flicker sessions (capture noise)', () => {
    // 2s Chrome flicker between two Code sessions is jitter, not a switch.
    expect(countSwitches([s('Code', 0, 600), s('Chrome', 10, 2), s('Code', 11, 600)], 15)).toBe(0)
  })

  it('orders by start time before counting', () => {
    expect(countSwitches([s('Chrome', 20, 600), s('Code', 0, 600)], 15)).toBe(1)
  })
})

// ---------------------------------------------------------------------------
// mergeIntervals — disjoint bands for the timeline
// ---------------------------------------------------------------------------

describe('mergeIntervals', () => {
  it('returns an empty list for no intervals', () => {
    expect(mergeIntervals([])).toEqual([])
  })

  it('leaves disjoint intervals untouched (count + span)', () => {
    const m = mergeIntervals([iv(0, 10), iv(20, 25)])
    expect(m.length).toBe(2)
    expect(unionSeconds(m)).toBe(900)
  })

  it('collapses overlapping intervals into one band', () => {
    const m = mergeIntervals([iv(0, 30), iv(10, 40)])
    expect(m.length).toBe(1)
    expect(unionSeconds(m)).toBe(2400)
  })

  it('merges touching intervals into one', () => {
    expect(mergeIntervals([iv(0, 10), iv(10, 20)]).length).toBe(1)
  })

  it('drops corrupt intervals before merging', () => {
    const m = mergeIntervals([iv(0, 10), { started_at: at(30), ended_at: at(5) }])
    expect(m.length).toBe(1)
  })
})

// ---------------------------------------------------------------------------
// intersectSeconds — agent ∩ active (supervised) vs outside (autonomous)
// ---------------------------------------------------------------------------

describe('clampIntervals', () => {
  it('drops intervals entirely outside the window and trims those that straddle it', () => {
    // window [10, 70] min. Three inputs: one before, one straddling lo, one past hi.
    const clamped = clampIntervals(
      [iv(0, 5), iv(0, 20), iv(60, 120)],
      at(10),
      at(70),
    )
    expect(clamped.length).toBe(2)
    expect(unionSeconds(clamped)).toBe(1200) // [10,20]=10m + [60,70]=10m
  })

  it('is a no-op on unparseable bounds (degrades safe, never zeroes)', () => {
    const ivs = [iv(0, 10)]
    expect(clampIntervals(ivs, 'not-a-date', at(70))).toEqual(ivs)
  })
})

describe('intersectSeconds', () => {
  it('returns 0 when sets do not overlap', () => {
    expect(intersectSeconds([iv(0, 10)], [iv(20, 30)])).toBe(0)
  })

  it('returns the overlapping span of two intervals', () => {
    // agent 10–40 over active 0–30 → supervised = 10–30 = 20 min
    expect(intersectSeconds([iv(10, 40)], [iv(0, 30)])).toBe(1200)
  })

  it('is symmetric', () => {
    expect(intersectSeconds([iv(10, 40)], [iv(0, 30)])).toBe(intersectSeconds([iv(0, 30)], [iv(10, 40)]))
  })

  it('sums overlap across multiple disjoint blocks', () => {
    // active in two blocks 0–10 and 20–30; agent spans 5–25 → overlaps 5–10 (5m) + 20–25 (5m)
    expect(intersectSeconds([iv(5, 25)], [iv(0, 10), iv(20, 30)])).toBe(600)
  })

  it('never exceeds either set (autonomous = agent − supervised ≥ 0)', () => {
    const agent = [iv(0, 60)]
    const active = [iv(10, 30), iv(40, 50)]
    const supervised = intersectSeconds(agent, active)
    const agentTotal = unionSeconds(agent)
    expect(agentTotal - supervised).toBeGreaterThanOrEqual(0)
    expect(supervised).toBe(1800) // 20m + 10m
  })
})
