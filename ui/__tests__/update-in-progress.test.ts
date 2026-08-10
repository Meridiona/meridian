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

/** Dashboard source, for the assertions that can only be made by scanning. */
const readSrc = (rel: string): string =>
  readFileSync(import.meta.dir + '/../' + rel, 'utf8')

type InvokeImpl = (cmd: string, args?: unknown) => Promise<unknown>
let invokeImpl: InvokeImpl = async () => null

// The popover's `listen()` registrations, captured so a test can deliver the
// events the Rust side really emits (`update-progress`, `update-finished`).
// A no-op `listen` would make every event-driven behaviour untestable — which
// is how the wedge below shipped unnoticed in the first place.
type Listener = (e: { payload: unknown }) => void
let listeners: Record<string, Listener[]> = {}

function emitEvent(name: string, payload: unknown) {
  for (const cb of listeners[name] || []) cb({ payload })
}

function loadPopover() {
  document.body.innerHTML = popoverBody()
  listeners = {}
  ;(globalThis as Record<string, unknown>).__MERIDIAN_POPOVER_TEST__ = true
  ;(globalThis as Record<string, unknown>).__TAURI__ = {
    core: { invoke: (cmd: string, args?: unknown) => invokeImpl(cmd, args) },
    event: {
      listen: async (name: string, cb: Listener) => {
        ;(listeners[name] ||= []).push(cb)
        return () => {}
      },
    },
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
  // Drain the microtask queue via a macrotask rather than counting `await`s:
  // a fixed number of turns silently stops covering the handler the moment an
  // extra `.then` is added to the chain.
  await new Promise((resolve) => setTimeout(resolve, 0))
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

  it('keeps tracking the running install and does not re-issue the install', async () => {
    let installCalls = 0
    invokeImpl = async (cmd) => {
      if (cmd === 'install_update') {
        installCalls++
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
    // Assert the COMMAND wasn't re-issued. Re-reading the label proves nothing
    // here - it reads the same whether or not the click was suppressed.
    expect(installCalls).toBe(1)
    expect(updText()?.toLowerCase()).toContain('in progress')

    // The point of staying in the installing state: the winner's progress is
    // broadcast to every window, so this banner keeps painting it.
    emitEvent('update-progress', { downloaded: 25, contentLength: 100 })
    expect(updText()).toContain('25%')
  })

  // The regression this whole recovery path exists for. Staying "installing"
  // is only safe while the winning install is alive; if it dies, the surface
  // that never received the Err must still be able to retry.
  it('recovers when the winning install fails, instead of wedging forever', async () => {
    let installCalls = 0
    invokeImpl = async (cmd) => {
      if (cmd === 'install_update') {
        installCalls++
        throw { kind: 'inProgress', message: 'An update is already being installed.' }
      }
      return null
    }
    await clickUpdate()
    expect(installCalls).toBe(1)

    // download_and_apply broadcasts this on its terminal `Failed` path.
    emitEvent('update-finished', { ok: false })
    expect(updText()).toBe('Update failed')

    // …and crucially the banner is live again: without this the retry button
    // is dead until the app is relaunched.
    await clickUpdate()
    expect(installCalls).toBe(2)
  })

  it('ignores a successful update-finished rather than flashing a failure', async () => {
    // The success path re-execs the app, so this should not normally be seen -
    // but an `ok: true` must never paint "Update failed".
    invokeImpl = async (cmd) => {
      if (cmd === 'install_update') {
        throw { kind: 'inProgress', message: 'An update is already being installed.' }
      }
      return null
    }
    await clickUpdate()
    emitEvent('update-finished', { ok: true })
    expect(updText()).not.toBe('Update failed')
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
  it('the dashboard card also recovers from a terminal failure', () => {
    // The card is React and this suite tests it through pure helpers rather
    // than rendering, so the subscription itself is guarded by source scan -
    // the same convention as `no-native-dialogs.test.ts`. Without it the card
    // is the surface left wedged, exactly as the popover was.
    const card = readSrc('components/timeline/UpdateCard.tsx')
    expect(card).toContain("'update-finished'")
    const handler = card.slice(card.indexOf("'update-finished'"))
    expect(handler).toContain('setInstalling(false)')
    expect(handler).toContain('setElsewhere(false)')
  })

  it('the popover discriminates on the same `inProgress` kind, not on prose', () => {
    // The popover is plain JS and cannot import from `ui/lib`, so the check is
    // necessarily written twice. This guards the copies against drifting apart.
    //
    // Matching the full comparison, not the bare string: `'inProgress'` alone
    // also appears in the explanatory comment above the predicate, so it would
    // still pass with the actual check deleted.
    expect(appSrc).toContain("kind === 'inProgress'")
  })

  // The Rust half of the contract is pinned where it can be asserted on the
  // real value rather than on source text: `update.rs`'s
  // `a_concurrent_install_is_refused_as_in_progress_not_as_a_failure` serialises
  // the error and asserts `"kind":"inProgress"`. A source regex over the
  // `rename_all` attribute was tried here and removed - any attribute added
  // between it and the enum (a `#[cfg]`, `#[non_exhaustive]`, a rustfmt reflow)
  // failed it while the wire shape was perfectly intact.
})
