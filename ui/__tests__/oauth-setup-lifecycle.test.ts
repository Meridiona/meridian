//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'fs'

// Regression guard for the OAuth "stuck on Waiting…" bug.
//
// `OAuthSetup` (components/IntegrationConnect.tsx) keeps a `mountedRef` so its
// async `startOAuth` body can detect an unmount that happened while awaiting
// `mutate('start_oauth')` — before the poll interval exists to be cleared:
//
//     await mutate(..., 'start_oauth', ...)   // backend runs, writes creds
//     if (!mountedRef.current) return          // bail if we unmounted meanwhile
//     ... create the 2s poll interval ...      // detects success + surfaces errors
//
// The original effect was cleanup-ONLY — it set the flag false on unmount but
// never set it true on (re)mount:
//
//     useEffect(() => () => { mountedRef.current = false; ... }, [])   // BUG
//
// `next.config.ts` has `reactStrictMode: true`, and `tauri dev` serves via the
// Next dev server, so React StrictMode runs effects mount→cleanup→mount. With a
// cleanup-only effect the first cleanup flips the flag false and the remount
// never restores it, leaving a LIVE component with `mountedRef.current === false`.
// `startOAuth` then bails right after the backend already wrote the credentials,
// the poll interval never starts, and the UI hangs on "Waiting…" forever — with
// any real OAuth error silently swallowed (the error path lives in that poll).
//
// The repo has no React render harness (no @testing-library/jsdom), so — like the
// other UI guards (cursor-pointer, NumberStepper) — we model the lifecycle and
// scan the source for the fixed shape rather than mount the component.

const uiRoot = import.meta.dir + '/..'
const src = readFileSync(uiRoot + '/components/IntegrationConnect.tsx', 'utf8')

type MountRef = { current: boolean }

// Faithful model of the FIXED effect: set the flag true on (re)mount, return a
// cleanup that sets it false. This is the contract the component must honour.
function fixedMountEffect(ref: MountRef): () => void {
  ref.current = true
  return () => { ref.current = false }
}

// The OLD buggy effect: body is JUST the cleanup, nothing re-arms the flag.
function buggyMountEffect(ref: MountRef): () => void {
  return () => { ref.current = false }
}

// Drive React's two lifecycle shapes against an effect.
function runNormalMount(effect: (r: MountRef) => () => void, ref: MountRef) {
  effect(ref) // single mount, cleanup runs only on real unmount
}
function runStrictModeMount(effect: (r: MountRef) => () => void, ref: MountRef) {
  const cleanup1 = effect(ref) // initial mount
  cleanup1() // StrictMode simulated unmount
  effect(ref) // StrictMode remount — component stays alive, no further cleanup
}

describe('OAuthSetup mount flag survives React StrictMode double-invoke', () => {
  it('fixed effect: flag is true after a normal single mount', () => {
    const ref: MountRef = { current: true }
    runNormalMount(fixedMountEffect, ref)
    expect(ref.current).toBe(true)
  })

  it('fixed effect: flag is true after StrictMode mount→cleanup→mount', () => {
    const ref: MountRef = { current: true }
    runStrictModeMount(fixedMountEffect, ref)
    expect(ref.current).toBe(true)
  })

  it('buggy effect: StrictMode leaves the flag false — proving the test discriminates', () => {
    // Documents the exact regression the fix prevents: with a cleanup-only
    // effect, a live component ends up with mountedRef.current === false.
    const ref: MountRef = { current: true }
    runStrictModeMount(buggyMountEffect, ref)
    expect(ref.current).toBe(false)
  })

  it('startOAuth guard does NOT bail when the (fixed) flag is true', () => {
    const ref: MountRef = { current: true }
    runStrictModeMount(fixedMountEffect, ref)
    // startOAuth: `if (!mountedRef.current) return` — true ⇒ the poll can start.
    const bailedBeforeCreatingPoll = !ref.current
    expect(bailedBeforeCreatingPoll).toBe(false)
  })

  it('startOAuth guard WOULD bail under the buggy effect (the user-visible hang)', () => {
    const ref: MountRef = { current: true }
    runStrictModeMount(buggyMountEffect, ref)
    const bailedBeforeCreatingPoll = !ref.current
    expect(bailedBeforeCreatingPoll).toBe(true)
  })
})

describe('IntegrationConnect.tsx OAuthSetup re-arms the mount flag', () => {
  it("the mount effect sets mountedRef.current = true BEFORE returning its cleanup", () => {
    // Must re-arm on (re)mount, not just clear on unmount. This regex fails for
    // the old cleanup-only `useEffect(() => () => { ... }, [])` shape.
    expect(src).toMatch(
      /useEffect\(\(\)\s*=>\s*\{[\s\S]*?mountedRef\.current\s*=\s*true[\s\S]*?return\s*\(\)\s*=>\s*\{/,
    )
  })

  it('still clears the flag on unmount and tears down the poll interval', () => {
    expect(src).toContain('mountedRef.current = false')
    expect(src).toMatch(/clearInterval\(pollRef\.current\)/)
  })

  it('startOAuth still guards on the flag after awaiting start_oauth', () => {
    expect(src).toMatch(/if \(!mountedRef\.current\) return/)
  })
})
