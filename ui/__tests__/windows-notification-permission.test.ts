//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'fs'

// Guards the Windows notification onboarding in the setup wizard. Windows has no
// TCC-style consent for Accessibility/Screen Recording, so those cards are
// macOS-only — but Notifications IS a real per-app setting on Windows (WinRT
// ToastNotifier::Setting), so the Permissions step must NOT be dropped there.
// It is swapped for a notifications-only variant that keeps id:'permissions' so
// page.tsx's check_notifications poll (gated on that id) still fires. This test
// is a string-source guard (the same pattern as the Settings → Notifications
// guard) because steps.tsx pulls in the whole React/Clerk import graph.
//
// The assertions are ANCHORED to the specific declaration they guard — an
// earlier version scanned the whole file with `[\s\S]*` and matched the macOS
// step's `id: 'permissions'` and the integrations step's `canNext: () => true`,
// so it passed against the very regressions it named. Each assertion below was
// verified to FAIL when the invariant it guards is mutated.
//
// Related backend: tray/src-tauri/src/commands/setup.rs (check_/request_notifications),
// tray/src-tauri/src/sys.rs (windows_permission_state), commands/system.rs
// (open_permission_pane "notifications" → ms-settings:notifications).

const uiRoot = import.meta.dir + '/..'
const readSrc = (rel: string): string => readFileSync(uiRoot + '/' + rel, 'utf8')

/** The body of a top-level `const NAME … = {` declaration: from the declaration
 *  to the first column-0 `}`. Scoped so an assertion can't match an unrelated
 *  step elsewhere in the file. */
function declBlock(src: string, marker: string): string {
  const start = src.indexOf(marker)
  if (start < 0) throw new Error(`marker not found: ${marker}`)
  return src.slice(start).split('\n}')[0]
}

describe('windows notification permission onboarding', () => {
  const steps = readSrc('app/setup/steps.tsx')

  it('keeps the Windows step under id:permissions so the poll still fires', () => {
    // page.tsx gates the check_notifications poll on steps[step].id === 'permissions'.
    const winStep = declBlock(steps, 'const WINDOWS_NOTIFICATIONS_STEP')
    expect(winStep).toContain("id: 'permissions'")
  })

  it('never blocks Continue on the optional Windows notifications card', () => {
    const winStep = declBlock(steps, 'const WINDOWS_NOTIFICATIONS_STEP')
    expect(winStep).toContain('canNext: () => true')
  })

  it('swaps the permissions step for the Windows variant instead of dropping it', () => {
    const build = steps.slice(steps.indexOf('export function buildSteps'))
    // The Windows branch must reference the variant (a swap)…
    expect(build).toContain('WINDOWS_NOTIFICATIONS_STEP')
    // …and must NOT filter the step list — the old drop, in any formatting.
    expect(build).not.toMatch(/\.filter\(/)
  })

  it('keeps a notifications entry the Windows card can render', () => {
    const notif = declBlock(readSrc('app/setup/data.ts'), "id: 'notifications'")
    expect(notif).toContain("pane: 'notifications'")
    expect(notif).toContain('required: false')
  })
})
