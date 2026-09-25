//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'fs'

// Guards the Windows notification onboarding in the setup wizard. Windows has no
// TCC-style consent for Accessibility/Screen Recording, so those cards are
// macOS-only — but Notifications IS a real per-app setting on Windows (WinRT
// ToastNotifier::Setting), so the Permissions step is swapped for a
// notifications-only variant that keeps id:'permissions' (page.tsx's
// check_notifications poll is gated on that id) rather than being dropped
// wholesale. It IS dropped in exactly one case — notifications already granted,
// where a step whose entire content is one satisfied toggle has nothing to ask —
// and the assertions below pin that condition rather than the branch. This test
// is a string-source guard (the same pattern as the Settings → Notifications
// guard) because steps.tsx pulls in the whole wizard's component/import graph
// (LlmProviderPicker, IntegrationConnect, the OTP form, …), too heavy to mount
// in this test suite's no-DOM bun test runner.
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

  it('swaps the permissions step for the Windows variant rather than losing it', () => {
    const build = steps.slice(steps.indexOf('export function buildSteps'))
    expect(build).toContain('WINDOWS_NOTIFICATIONS_STEP')
  })

  // The step IS dropped on Windows now — but only when notifications are
  // ALREADY granted, which is the one case where it has nothing to ask for
  // (Windows enables toasts for most apps by default). This used to assert the
  // step could never be dropped at all; that was the right guard when the only
  // alternative on the table was losing Windows notification onboarding
  // entirely, and it is the wrong one now. What still must never happen is the
  // UNCONDITIONAL drop, so this pins the condition instead of forbidding the
  // branch: not-granted, unavailable, and unresolved all keep the step.
  it('only drops the Windows step when notifications are already granted', () => {
    const build = steps.slice(steps.indexOf('export function buildSteps'))
    expect(build).toContain('notificationsGranted')
    // The empty-list arm (the drop) must be reachable only through that flag.
    expect(build).toMatch(/notificationsGranted \? \[\] : \[WINDOWS_NOTIFICATIONS_STEP\]/)
  })

  // The flag feeding that branch must be a ONE-SHOT read taken before the user
  // leaves Welcome — not the 2 s poll, which only runs while the Permissions
  // step is already on screen and so cannot answer whether it should be. And
  // the wizard must not let the user past Welcome until it has landed, or the
  // step list could reshape under a step index already navigated to.
  it('decides from a precheck resolved before Welcome is dismissed', () => {
    const page = readSrc('app/setup/page.tsx')
    expect(page).toContain("invoke<NotifState>('check_notifications')")
    expect(page).toContain('buildSteps(platform, notifPrecheck === \'granted\')')
    expect(page).toContain('ready={platform !== null && notifPrecheck !== null}')
  })

  // A failed probe must not read as a grant — that would silently skip
  // notification onboarding because one IPC call went wrong.
  it('keeps the step when the precheck fails', () => {
    const page = readSrc('app/setup/page.tsx')
    const precheck = page.slice(page.indexOf("invoke<NotifState>('check_notifications')"))
    expect(precheck.slice(0, 400)).toContain("'unavailable'")
  })

  it('keeps a notifications entry the Windows card can render', () => {
    const notif = declBlock(readSrc('app/setup/data.ts'), "id: 'notifications'")
    expect(notif).toContain("pane: 'notifications'")
  })

  // `required` on a PermissionMeta is BADGE COPY ONLY — it picks the
  // REQUIRED/OPTIONAL chip on the card. It is deliberately not wired to any
  // gate: what can actually block Continue is the step's own `canNext`, which
  // on Windows is unconditional (asserted above) and on macOS checks only
  // accessibility + screen.
  //
  // This used to assert `required: false`, which read as "notifications cannot
  // block setup" — but it was pinning the chip's wording to prove a gating
  // property it never controlled. Flipping the chip to REQUIRED (a deliberate
  // copy change) then failed a test about gating, for a reason unrelated to
  // gating. This asserts the decoupling itself instead.
  it('never lets the notifications card gate a step, whatever its badge says', () => {
    const data = readSrc('app/setup/data.ts')
    const steps = readSrc('app/setup/steps.tsx')
    const notif = declBlock(data, "id: 'notifications'")
    // Whichever way the badge reads, it must be a bare literal — not derived
    // from live permission state, which would make it a gate in disguise.
    expect(notif).toMatch(/required: (true|false),/)
    // And no step's canNext may consult notification state.
    const macGate = declBlock(steps, 'const PERMISSIONS_STEP')
    expect(macGate).toContain('canNext:')
    expect(macGate).not.toContain('perms.notifications')
  })
})
