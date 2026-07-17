//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'fs'

// Regression guard for the OAuth "stuck on Waiting…" bug.
//
// THE BUG: `OAuthSetup` kicked off the flow and then polled for completion:
//
//     await mutate(..., 'start_oauth', ...)   // backend runs, writes creds
//     ... create the 2s poll interval ...      // detects success + surfaces errors
//
// With the flow's state and poll interval owned by the COMPONENT, an unmount
// between those two lines orphaned the flow: the backend had already written the
// credentials, but the poll that detects success never started, so the UI hung on
// "Waiting…" forever with any real OAuth error silently swallowed (the error path
// lives in that poll). `next.config.ts` sets `reactStrictMode: true` and `tauri
// dev` serves via the Next dev server, so React runs effects mount→cleanup→mount
// and made this fire on the FIRST connect attempt every time.
//
// THE OLD FIX was a `mountedRef` the async body re-checked after the await, plus
// an effect that re-armed the flag on remount. It worked, but it only patched the
// StrictMode case — a real unmount (collapsing the accordion row, switching
// provider, closing Settings) still threw the flow away mid-flight.
//
// THE FIX IS NOW STRUCTURAL: the state and the poll loop live in a module-level,
// provider-keyed store (components/integrationConnectStore.ts) that OUTLIVES the
// component. The panel is a thin `useSyncExternalStore` view. Unmounting — for any
// reason — can no longer orphan an in-flight flow, and reopening the row picks the
// live flow back up. So `mountedRef` is not merely unnecessary, its return would
// signal the state has been pulled back into the component. These guards pin that.
//
// The repo has no React render harness (no @testing-library/jsdom), so — like the
// other UI guards (cursor-pointer, NumberStepper) — we model the lifecycle and
// scan the source for the required shape rather than mount the component.

const uiRoot = import.meta.dir + '/..'
const view = readFileSync(uiRoot + '/components/IntegrationConnect.tsx', 'utf8')
const store = readFileSync(uiRoot + '/components/integrationConnectStore.ts', 'utf8')

// ── The lifecycle model: where state lives decides whether it survives ────────

/** Component-local state (the OLD shape): a fresh object per mount, thrown away
 *  by cleanup. This is what made an in-flight flow droppable. */
function componentLocalFlow() {
  let state: { status: string } | null = null
  return {
    mount: () => { state = { status: 'idle' } },
    cleanup: () => { state = null },
    start: () => { if (state) state.status = 'waiting' },
    read: () => state?.status ?? 'lost',
  }
}

/** Module-level keyed store (the CURRENT shape): mount/cleanup are just
 *  subscribe/unsubscribe — they never touch the flow's state. */
function moduleStoreFlow() {
  const map = new Map<string, { status: string }>()
  const listeners = new Set<() => void>()
  return {
    mount: () => { listeners.add(() => {}) },
    cleanup: () => { listeners.clear() },
    start: () => { map.set('jira', { status: 'waiting' }) },
    read: () => map.get('jira')?.status ?? 'lost',
  }
}

describe('an in-flight OAuth flow survives unmount', () => {
  it('module store: a StrictMode mount→cleanup→mount keeps the flow', () => {
    const f = moduleStoreFlow()
    f.mount(); f.cleanup(); f.mount()   // StrictMode double-invoke
    f.start()
    expect(f.read()).toBe('waiting')
  })

  it('module store: a REAL unmount mid-flight keeps the flow, so reopening resumes it', () => {
    const f = moduleStoreFlow()
    f.mount()
    f.start()                            // flow running…
    f.cleanup()                          // …row collapsed / provider switched
    expect(f.read()).toBe('waiting')     // still there
    f.mount()                            // reopened — picks the live flow back up
    expect(f.read()).toBe('waiting')
  })

  it('component-local: the same unmount LOSES the flow — proving these discriminate', () => {
    // Documents the exact regression the store prevents: the backend has written
    // the creds, but the state tracking it is gone, so the UI hangs on "Waiting…".
    const f = componentLocalFlow()
    f.mount()
    f.start()
    f.cleanup()
    expect(f.read()).toBe('lost')
  })
})

// ── The source shape that backs the model ────────────────────────────────────

describe('integrationConnectStore.ts owns the OAuth flow', () => {
  it('keeps the poll loop at module scope, keyed by provider', () => {
    // Module-level (not inside a component/hook) is what makes it outlive unmount.
    expect(store).toMatch(/^const pollers = new Map</m)
    expect(store).toMatch(/^function stopPoll\(provider: string\)/m)
  })

  it('exposes the flow as functions over the store, not component state', () => {
    expect(store).toMatch(/^export async function startOAuth\(/m)
    expect(store).toMatch(/^export function cancelOAuth\(/m)
  })

  it('hands useSyncExternalStore a STABLE empty snapshot', () => {
    // A fresh object per getSnapshot call re-renders forever; the shared frozen
    // default is what stops that.
    expect(store).toContain('useSyncExternalStore')
    expect(store).toMatch(/const OAUTH_EMPTY: OAuthState = Object\.freeze\(/)
  })
})

describe('IntegrationConnect.tsx stays a thin view over the store', () => {
  it('reads OAuth state from the store rather than holding it locally', () => {
    expect(view).toMatch(/useConnectStore\(oauthStore, tracker\.id\)/)
  })

  it('has no mountedRef / component-owned poll — the bug those patched is gone', () => {
    // Their return means the flow's state moved back into the component, which is
    // exactly what made an unmount able to orphan it.
    expect(view).not.toContain('mountedRef')
    expect(view).not.toContain('pollRef')
    expect(view).not.toContain('setInterval')
  })
})
