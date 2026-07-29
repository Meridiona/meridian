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
// Related backend: tray/src-tauri/src/commands/setup.rs (check_/request_notifications),
// tray/src-tauri/src/sys.rs (windows_permission_state), commands/system.rs
// (open_permission_pane "notifications" → ms-settings:notifications).

const uiRoot = import.meta.dir + '/..'
const readSrc = (rel: string): string => readFileSync(uiRoot + '/' + rel, 'utf8')

describe('windows notification permission onboarding', () => {
  const steps = readSrc('app/setup/steps.tsx')

  it('does not drop the whole permissions step on Windows', () => {
    // The old behaviour filtered the step out entirely — the regression this
    // feature reverses. If this reappears, Windows loses notification onboarding.
    expect(steps).not.toContain("filter((s) => s.id !== 'permissions')")
  })

  it('swaps in a notifications-only variant that keeps id:permissions', () => {
    expect(steps).toContain('WINDOWS_NOTIFICATIONS_STEP')
    // buildSteps must map the permissions step to the Windows variant, not remove it.
    expect(steps).toMatch(/platform === 'windows'[\s\S]*s\.id === 'permissions'[\s\S]*WINDOWS_NOTIFICATIONS_STEP/)
    // Same id as macOS so the check_notifications poll (page.tsx gates it on
    // steps[step].id === 'permissions') keeps firing on Windows.
    expect(steps).toMatch(/WINDOWS_NOTIFICATIONS_STEP[\s\S]*id: 'permissions'/)
  })

  it('never blocks Continue on the optional Windows notifications card', () => {
    // Notifications is optional on both OSes; on Windows it is the only card, so
    // the step must not gate navigation.
    expect(steps).toMatch(/WINDOWS_NOTIFICATIONS_STEP[\s\S]*canNext: \(\) => true/)
  })

  it('keeps a notifications entry the Windows card can render', () => {
    const data = readSrc('app/setup/data.ts')
    expect(data).toMatch(/id: 'notifications'[\s\S]*pane: 'notifications'/)
    expect(data).toContain('required: false')
  })
})
