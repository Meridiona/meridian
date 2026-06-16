//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, it, expect } from 'bun:test'
import { meridianCandidates, selectMeridianBinary } from '../lib/meridian-bin'

// ---------------------------------------------------------------------------
// Candidate ordering — the native binary must outrank the node wrapper.
//
// Regression guard for the dashboard "Sync failed: env: node: No such file or
// directory" bug: under launchd's stripped PATH the `#!/usr/bin/env node`
// wrapper at ~/.local/bin/meridian can't resolve node, while the native
// Mach-O binary at ~/.meridian/app/bin/meridian has no runtime deps. If the
// ordering ever regresses so the wrapper is probed first, this test fails.
// ---------------------------------------------------------------------------

const HOME = '/home/u'
const native = `${HOME}/.meridian/app/bin/meridian`
const systemNative = '/usr/local/bin/meridian'
const nodeWrapper = `${HOME}/.local/bin/meridian`

describe('meridianCandidates', () => {
  it('lists the native binary before the node wrapper', () => {
    const list = meridianCandidates(HOME)
    expect(list.indexOf(native)).toBeLessThan(list.indexOf(nodeWrapper))
  })

  it('lists the node wrapper last', () => {
    const list = meridianCandidates(HOME)
    expect(list[list.length - 1]).toBe(nodeWrapper)
  })

  it('expands HOME into every home-relative candidate', () => {
    const list = meridianCandidates(HOME)
    expect(list).toContain(native)
    expect(list).toContain(nodeWrapper)
    expect(list.some(p => p.includes('undefined'))).toBe(false)
  })
})

describe('meridianCandidates in development', () => {
  // `npm run dev` runs with cwd = <repo>/ui, so the repo root is one level up.
  const list = meridianCandidates(HOME, 'development', '/work/meridian/ui')

  it('prepends the repo build (target/debug) ahead of the installed binaries', () => {
    expect(list[0]).toBe('/work/meridian/target/debug/meridian')
    expect(list.indexOf('/work/meridian/target/debug/meridian')).toBeLessThan(list.indexOf(native))
  })

  it('keeps a release build as a fallback after debug but before installed', () => {
    const debug = list.indexOf('/work/meridian/target/debug/meridian')
    const release = list.indexOf('/work/meridian/target/release/meridian')
    expect(debug).toBeLessThan(release)
    expect(release).toBeLessThan(list.indexOf(native))
  })

  it('still lists the node wrapper last (launchd-parity ordering preserved)', () => {
    expect(list[list.length - 1]).toBe(nodeWrapper)
  })

  it('does not prepend repo builds outside development', () => {
    expect(meridianCandidates(HOME, 'production', '/work/meridian/ui')[0]).toBe(native)
  })
})

describe('selectMeridianBinary', () => {
  const candidates = meridianCandidates(HOME, 'production')

  it('prefers the native binary when both native and wrapper are executable', () => {
    // Everything executable (the real dev-machine case that masked the bug).
    const bin = selectMeridianBinary(candidates, () => true)
    expect(bin).toBe(native)
  })

  it('falls back to the node wrapper when only the wrapper exists', () => {
    const bin = selectMeridianBinary(candidates, p => p === nodeWrapper)
    expect(bin).toBe(nodeWrapper)
  })

  it('skips the absent native binary and takes the next executable native path', () => {
    const bin = selectMeridianBinary(candidates, p => p === systemNative || p === nodeWrapper)
    expect(bin).toBe(systemNative)
  })

  it('returns the first candidate when nothing is executable (meaningful ENOENT, not undefined)', () => {
    const bin = selectMeridianBinary(candidates, () => false)
    expect(bin).toBe(native)
  })

  it('never returns undefined', () => {
    expect(selectMeridianBinary(candidates, () => false)).toBeDefined()
  })
})
