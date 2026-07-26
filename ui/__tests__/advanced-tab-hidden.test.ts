//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'fs'

// Guards the "Advanced" settings tab being hidden from the sidebar/modal, and
// specifically the regression a review caught: hiding it must NOT also hide
// "Export Diagnostics" — a packaged install's only GUI path to handing a
// developer a local telemetry bundle (see CLAUDE.md's "Telemetry: local-only
// capture, dev-only shipping"). That control was moved to AccountSection.tsx,
// which stays reachable, rather than dropped.

const uiRoot = import.meta.dir + '/..'
const readSrc = (rel: string): string => readFileSync(uiRoot + '/' + rel, 'utf8')

describe('Advanced tab is hidden, not deleted', () => {
  it('the sidebar nav entry is commented out, not removed', () => {
    const src = readSrc('components/timeline/settings/SettingsSidebar.tsx')
    expect(src).toContain("// { id: 'advanced'")
    expect(src).not.toMatch(/^\s*\{\s*id:\s*'advanced'/m)
  })

  it("the modal's render branch is commented out, not removed", () => {
    const src = readSrc('components/timeline/SettingsModal.tsx')
    expect(src).toContain("{/* {section === 'advanced' && (")
    expect(src).not.toMatch(/^\s*\{section === 'advanced' && \(/m)
  })
})

describe('Export Diagnostics stays reachable via Account', () => {
  const accountSrc = readSrc('components/timeline/settings/AccountSection.tsx')
  const advancedSrc = readSrc('components/timeline/settings/AdvancedSection.tsx')

  it('AccountSection wires the export button through useExportDiagnostics', () => {
    expect(accountSrc).toContain("import { useExportDiagnostics } from './useExportDiagnostics'")
    expect(accountSrc).toContain('exportBundle')
    expect(accountSrc).toContain('Export Diagnostics')
  })

  it('AdvancedSection no longer duplicates the export button', () => {
    // A doc comment may still mention it moved; the functional wiring must not.
    expect(advancedSrc).not.toContain('useExportDiagnostics(')
    expect(advancedSrc).not.toContain('exportBundle')
    expect(advancedSrc).not.toContain('<SettingsButton onClick={exportBundle}')
  })
})
