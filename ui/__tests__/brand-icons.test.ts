//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, it, expect } from 'bun:test'
import { normalizeAppName, getBrandIcon, BRAND_ICONS } from '../lib/brand-icons'

describe('normalizeAppName', () => {
  it("strips a leading left-to-right mark (WhatsApp's real macOS app_name)", () => {
    expect(normalizeAppName('\u200EWhatsApp')).toBe('WhatsApp')
  })

  it('strips zero-width space/joiner/non-joiner and a BOM', () => {
    expect(normalizeAppName('\u200BSlack')).toBe('Slack')
    expect(normalizeAppName('\u200CFigma')).toBe('Figma')
    expect(normalizeAppName('\u200DNotion')).toBe('Notion')
    expect(normalizeAppName('\uFEFFZoom')).toBe('Zoom')
  })

  it('leaves a plain name untouched', () => {
    expect(normalizeAppName('Claude Code')).toBe('Claude Code')
  })

  it('trims surrounding whitespace', () => {
    expect(normalizeAppName('  Arc  ')).toBe('Arc')
  })
})

describe('getBrandIcon', () => {
  it('resolves WhatsApp even with the real hidden LRM prefix macOS reports', () => {
    expect(getBrandIcon('\u200EWhatsApp')?.hex).toBe(BRAND_ICONS.WhatsApp.hex)
  })

  it('resolves a plain exact match (Claude Code)', () => {
    expect(getBrandIcon('Claude Code')?.hex).toBe('#D97757')
  })

  it('resolves hex-only entries (no vector path) for ChatGPT and VS Code', () => {
    expect(getBrandIcon('ChatGPT')).toEqual({ hex: '#000000' })
    expect(getBrandIcon('Code')).toEqual({ hex: '#007ACC' })
  })

  it('returns undefined for an unmapped app', () => {
    expect(getBrandIcon('Some Unmapped App')).toBeUndefined()
  })

  it('returns undefined for null/undefined input', () => {
    expect(getBrandIcon(null)).toBeUndefined()
    expect(getBrandIcon(undefined)).toBeUndefined()
  })
})
