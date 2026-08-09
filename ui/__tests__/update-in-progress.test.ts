//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Regression guard for the "Update failed" label that isn't a failure.
//
// # The field incident (1.84.0-staging.5 → .7, 2026-08-09)
// The DMG updater is driven from TWO surfaces that are routinely open at the
// same time: the tray popover's `.upd` banner and the dashboard sidebar's
// UpdateCard. Both call the same `install_update` command, which is guarded by
// a single-flight `INSTALLING` swap in `tray/src-tauri/src/update.rs` — so
// clicking the second surface while the first is downloading returns
// `Err(kind: inProgress)`.
//
// Both surfaces treated *any* rejection as a failed install. The measured
// result: the popover rendered "Update failed" and the dashboard offered
// "Click to try again", while the install those two clicks had actually
// started completed fine and relaunched the app 36 s later. The user sees a
// failure that did not happen, and the retry it invites can only be rejected
// again for as long as the real install runs.
//
// # What this locks in
// A rejection carrying `kind === 'inProgress'` must NEVER render as a failure
// on either surface — it means "someone else is already installing", so the
// banner tracks that install instead. A rejection of any other shape (a real
// pre-relaunch error, or an IPC-level throw that isn't our error object at
// all) must still render as a failure, or a genuinely broken update would go
// silent.
//
// # Why a DOM harness for the popover
// The popover is plain HTML+JS in a menu-bar NSPanel; there is no WebDriver
// path to it on macOS. So, exactly as `popover-health-panel.test.ts` does, the
// REAL `tray/src/app.js` is loaded into happy-dom over the REAL markup from
// `index.html`, with a mocked `__TAURI__` bridge. The click handler is
// registered at module scope, above the `boot()` guard, so it is live under
// the test flag without booting the ticker.

import { describe, it, expect, beforeAll, afterAll, beforeEach } from 'bun:test'
import { readFileSync } from 'fs'
import { GlobalRegistrator } from '@happy-dom/global-registrator'
import { isInstallInProgress } from '../components/timeline/UpdateCard'

const trayDir = import.meta.dir + '/../../tray/src'

// Same extraction as popover-health-panel.test.ts: the real markup, minus the
// <script> tags we load by hand.
function stripScriptTags(html: string): string {
  let out = html
  let prev: string
  do {
    prev = out
    out = out.replace(/<script\b[^>]*>[\s\S]*?<\/script(?:\s+[^>]*)?\s*>/gi, '')
  } while (out !== prev)
  return out
}

function popoverBody(): string {
  const html = readFileSync(trayDir + '/index.html', 'utf8')
  const m = html.match(/<body[^>]*>([\s\S]*)<\/body>/i)
  return stripScriptTags(m ? m[1] : '')
}

const pauseUtilsSrc = readFileSync(trayDir + '/pause-utils.js', 'utf8')
const appSrc = readFileSync(trayDir + '/app.js', 'utf8')

type InvokeImpl = (cmd: string, args?: unknown) => Promise<unknown>
let invokeImpl: InvokeImpl = async () => null

function loadPopover() {
  document.body.innerHTML = popoverBody()
  ;(globalThis as Record<string, unknown>).__MERIDIAN_POPOVER_TEST__ = true
  ;(globalThis as Record<string, unknown>).__TAURI__ = {
    core: { invoke: (cmd: string, args?: unknown) => invokeImpl(cmd, args) },
    event: { listen: async () => () => {} },
    window: {
      getCurrentWindow: () => ({ setSize: async () => {} }),
      LogicalSize: class {
        constructor(
          public w: number,
          public h: number,
        ) {}
      },
    },
  }
  ;(0, eval)(pauseUtilsSrc + '\n' + appSrc)
}

const updText = () => document.getElementById('upd-text')?.textContent

/** Click the banner and let the rejected `install_update` promise settle. */
async function clickUpdate() {
  document.getElementById('upd')?.dispatchEvent(new Event('click'))
  // Two turns: one for the invoke promise, one for the .catch handler.
  await Promise.resolve()
  await Promise.resolve()
}

beforeAll(() => GlobalRegistrator.register())
afterAll(() => GlobalRegistrator.unregister())

beforeEach(() => {
  invokeImpl = async () => null
  loadPopover()
})

// ── The popover banner ───────────────────────────────────────────────────────

describe('popover update banner', () => {
  it('does NOT say "Update failed" when another surface is already installing', async () => {
    invokeImpl = async (cmd) => {
      if (cmd === 'install_update') {
        // Exactly what the Rust single-flight guard rejects with.
        throw { kind: 'inProgress', message: 'An update is already being installed.' }
      }
      return null
    }
    await clickUpdate()

    // The install is real and running; calling that a failure is the bug.
    expect(updText()).not.toBe('Update failed')
    expect(updText()?.toLowerCase()).toContain('in progress')
  })

  it('keeps tracking the running install so progress events still land', async () => {
    invokeImpl = async (cmd) => {
      if (cmd === 'install_update') {
        throw { kind: 'inProgress', message: 'An update is already being installed.' }
      }
      return null
    }
    await clickUpdate()

    // The banner must stay in the installing state: the winning surface's
    // `update-progress` events are broadcast to every window, and the
    // popover's listener is gated on that flag. Dropping out of it would
    // freeze this banner on a stale label for the whole download.
    expect(document.getElementById('upd')?.textContent).not.toContain('failed')
    await clickUpdate()
    // A second click must not re-issue the install while one is known to run.
    expect(updText()?.toLowerCase()).toContain('in progress')
  })

  it('still reports a genuine pre-relaunch failure as a failure', async () => {
    invokeImpl = async (cmd) => {
      if (cmd === 'install_update') {
        throw { kind: 'failed', message: 'signature verification failed' }
      }
      return null
    }
    await clickUpdate()
    expect(updText()).toBe('Update failed')
  })

  it('treats a non-object rejection (IPC-level throw) as a failure', async () => {
    // If the bridge itself dies the rejection is an Error, not our shape.
    // Defaulting that to "in progress" would hide a broken updater.
    invokeImpl = async (cmd) => {
      if (cmd === 'install_update') throw new Error('ipc closed')
      return null
    }
    await clickUpdate()
    expect(updText()).toBe('Update failed')
  })
})

// ── The dashboard card's discriminator ───────────────────────────────────────
//
// UpdateCard is a React component; following this suite's convention
// (`staleAmount`, `phaseFor`, `canPickProposalProvider`), the decision is
// extracted as a pure predicate and tested directly rather than rendered.

describe('isInstallInProgress', () => {
  it('recognises the single-flight rejection', () => {
    expect(
      isInstallInProgress({ kind: 'inProgress', message: 'An update is already being installed.' }),
    ).toBe(true)
  })

  it('rejects a real failure, so it still surfaces as one', () => {
    expect(isInstallInProgress({ kind: 'failed', message: 'boom' })).toBe(false)
  })

  it('rejects shapes it does not understand rather than guessing', () => {
    // Anything that isn't our error object gets the safe reading: a failure.
    expect(isInstallInProgress(new Error('ipc closed'))).toBe(false)
    expect(isInstallInProgress('An update is already being installed.')).toBe(false)
    expect(isInstallInProgress(null)).toBe(false)
    expect(isInstallInProgress(undefined)).toBe(false)
    expect(isInstallInProgress({})).toBe(false)
  })
})

// ── Both surfaces must agree on the discriminant ─────────────────────────────

describe('the two surfaces stay in lockstep', () => {
  it('both match on the same `inProgress` kind the Rust guard emits', () => {
    // The popover is plain JS and cannot import from `ui/lib`, so the check is
    // necessarily written twice. This is the guard against the copies drifting
    // apart — and against either drifting from the Rust `UpdateErrorKind`.
    expect(appSrc).toContain("'inProgress'")

    const rustSrc = readFileSync(
      import.meta.dir + '/../../tray/src-tauri/src/update.rs',
      'utf8',
    )
    // `#[serde(rename_all = "camelCase")]` on the enum is what makes the
    // variant serialise as `inProgress`; without it both surfaces go blind.
    expect(rustSrc).toContain('enum UpdateErrorKind')
    expect(rustSrc).toMatch(/rename_all = "camelCase"\)\]\s*pub enum UpdateErrorKind/)
  })
})
