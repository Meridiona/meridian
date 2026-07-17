//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'fs'

// Guards the dashboard DMG update card — the sibling of the tray popover's
// `.upd` banner. It must reuse the exact same Rust commands (so both surfaces
// behave identically), stay consent-based (render only when an update is
// actually available), degrade safely outside Tauri, and be mounted in the
// sidebar (OverviewPanel), not as a full-width header banner.

const uiRoot = import.meta.dir + '/..'
const readSrc = (rel: string): string => readFileSync(uiRoot + '/' + rel, 'utf8')

// ── Mounted in the sidebar overview, not the global header ───────────────────

describe('UpdateCard placement', () => {
  it('is mounted in the OverviewPanel sidebar', () => {
    const src = readSrc('components/timeline/OverviewPanel.tsx')
    expect(src).toContain("import { UpdateCard } from './UpdateCard'")
    expect(src).toContain('<UpdateCard />')
  })

  it('is NOT a full-width header banner in LayoutBanners', () => {
    const src = readSrc('components/LayoutBanners.tsx')
    expect(src).not.toContain('UpdateCard')
    expect(src).not.toContain('UpdateBanner')
  })
})

// ── Reuses the popover's commands and stays consent-based ────────────────────

describe('UpdateCard', () => {
  const src = readSrc('components/timeline/UpdateCard.tsx')

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

  it('is styled as an accent card (matches the cleanup CTA), not a bar', () => {
    expect(src).toContain('rounded-xl')
    expect(src).toContain('var(--accent)')
  })

  it('resets the stale download percentage when retrying after a failed install', () => {
    const onInstall = src.slice(src.indexOf('const onInstall'), src.indexOf('const title'))
    expect(onInstall).toContain('setPct(null)')
  })
})
