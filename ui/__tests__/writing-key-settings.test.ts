//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'fs'

// The writing key is ON by default and reads on-screen text, so two things must not drift:
// the defaults the UI assumes must equal the Rust defaults (a mismatch would show a switch
// off while the tray treats it as on), and the master switch must stay reachable in Settings.

const root = import.meta.dir + '/../..'
const read = (rel: string) => readFileSync(root + '/' + rel, 'utf8')

const KEYS = ['compose_enabled', 'compose_sound', 'compose_typing_dots'] as const

describe('writing key settings', () => {
  const ts = read('ui/lib/settings.ts')
  const rust = read('meridian-core/src/settings.rs')

  it('defaults every switch to on in the UI', () => {
    for (const key of KEYS) {
      expect(ts).toMatch(new RegExp(`${key}:\\s*true,`))
    }
  })

  it('defaults every switch to on in the Rust settings, matching the UI', () => {
    for (const key of KEYS) {
      expect(rust).toMatch(new RegExp(`${key}:\\s*true,`))
    }
  })

  it('declares every switch on the RuntimeSettings type', () => {
    for (const key of KEYS) {
      expect(ts).toMatch(new RegExp(`${key}:\\s*boolean`))
    }
  })

  it('keeps the master switch in Capture & Privacy', () => {
    const section = read('ui/components/timeline/settings/CaptureSection.tsx')
    expect(section).toContain('<WritingKeyCard')
    const card = read('ui/components/timeline/settings/WritingKeyCard.tsx')
    expect(card).toContain('settings.compose_enabled')
    expect(card).toContain("compose_enabled: v")
  })

  it('uses plain hyphens in the card copy', () => {
    const card = read('ui/components/timeline/settings/WritingKeyCard.tsx')
    expect(card).not.toMatch(/[–—]/)
  })
})
