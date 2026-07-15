//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'fs'

// Guards the dashboard DMG update banner — the sibling of the tray popover's
// `.upd` banner. It must reuse the exact same Rust commands (so both surfaces
// behave identically), stay consent-based (render only when an update is
// actually available), degrade safely outside Tauri, and be mounted on every
// non-setup route via LayoutBanners.

const uiRoot = import.meta.dir + '/..'
const readSrc = (rel: string): string => readFileSync(uiRoot + '/' + rel, 'utf8')

// ── LayoutBanners mounts it, gated off the wizard ────────────────────────────

describe('LayoutBanners mounts the update banner', () => {
  const src = readSrc('components/LayoutBanners.tsx')

  it('renders <UpdateBanner /> alongside the fault-notice bar', () => {
    expect(src).toContain("import UpdateBanner from '@/components/UpdateBanner'")
    expect(src).toContain('<UpdateBanner />')
  })

  it('gates every banner off the setup wizard (self-contained onboarding shell)', () => {
    expect(src).toContain("pathname?.startsWith('/setup')")
    expect(src).toContain('return null')
  })
})

// ── UpdateBanner reuses the popover's commands and stays consent-based ────────

describe('UpdateBanner', () => {
  const src = readSrc('components/UpdateBanner.tsx')

  it('checks via `check_update` and installs via `install_update`', () => {
    expect(src).toContain("invoke<UpdateStatus>('check_update')")
    expect(src).toContain("invoke('install_update')")
  })

  it('subscribes to the `update-progress` event for the live percentage', () => {
    expect(src).toContain("'update-progress'")
  })

  it('renders only when an update is actually available (consent-based)', () => {
    // Keeps the "available" result; anything else leaves status null → no render.
    expect(src).toContain("s.state === 'available'")
    expect(src).toContain('if (!status) return null')
  })

  it('no-ops outside Tauri instead of throwing (browser-safe)', () => {
    expect(src).toContain('if (!isTauri()) return')
  })
})
