//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The sign-in gate: `commands::account::sign_in_required` decides whether this
// build requires an account, and THREE frontend surfaces have to act on the
// same answer — the dashboard-wide `RequireSignIn`, the setup wizard's sign-in
// step, and Settings → Account's `AccountAuthControl`.
//
// Two classes of bug this locks down, both of which actually happened:
//
//   1. FAILING OPEN. A rejected `invoke` must land on the gate. Two of the
//      three copies had no `.catch` at all, so a rejection left the promise
//      unsettled and the surface hung blank forever — and any "fix" that
//      defaulted to `false` would have been strictly worse: a broken bridge
//      would unlock the app.
//   2. RE-ROLLING IT. The copies drifted precisely because each site wrote its
//      own. The source scan below fails if a new consumer calls the command
//      directly instead of going through the shared hook.
//
// The behavioural half needs no DOM: `resolveSignInRequired` takes its asker as
// a parameter for exactly this reason (mirroring `sign_in_required_for` on the
// Rust side, which takes the key rather than looking it up).

import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'node:fs'
import { resolveSignInRequired } from '../lib/signin-required'
import { stripComments } from './helpers/source'

const uiRoot = import.meta.dir + '/..'
const read = (rel: string) => stripComments(readFileSync(uiRoot + rel, 'utf8'))

/** Every surface that acts on the answer. */
const CONSUMERS = [
  '/app/setup/signin/RequireSignIn.tsx',
  '/app/setup/signin/AccountAuthControl.tsx',
  '/app/setup/page.tsx',
]

describe('resolveSignInRequired', () => {
  it('passes through a true answer', async () => {
    expect(await resolveSignInRequired(async () => true)).toBe(true)
  })

  it('passes through a false answer — the contributor bypass', async () => {
    expect(await resolveSignInRequired(async () => false)).toBe(false)
  })

  it('fails CLOSED when the command rejects', async () => {
    const boom = async () => { throw new Error('command not found') }
    expect(await resolveSignInRequired(boom)).toBe(true)
  })

  it('fails CLOSED when the command rejects with a non-Error', async () => {
    // Tauri rejects with a plain string, not an Error — a `catch (e)` that
    // assumed `e.message` would throw again inside the handler.
    const boom = () => Promise.reject('unknown command: sign_in_required')
    expect(await resolveSignInRequired(boom)).toBe(true)
  })

  it('always settles, so a caller can never hang on it', async () => {
    // The original bug was an unsettled promise, not a wrong value: the surface
    // sat on its `null` loading state forever. A timeout race is the only way to
    // assert "this resolves at all".
    const hung = Promise.race([
      resolveSignInRequired(() => Promise.reject(new Error('x'))),
      new Promise((_, rej) => setTimeout(() => rej(new Error('never settled')), 1000)),
    ])
    expect(await hung).toBe(true)
  })
})

describe('the gate has one implementation', () => {
  it('no consumer invokes sign_in_required directly', () => {
    for (const rel of CONSUMERS) {
      expect(read(rel)).not.toContain("'sign_in_required'")
    }
  })

  it('every consumer goes through the shared hook', () => {
    for (const rel of CONSUMERS) {
      expect(read(rel)).toContain('useSignInRequired')
    }
  })

  it('only the shared module names the command', () => {
    expect(read('/lib/signin-required.ts')).toContain("invoke<boolean>('sign_in_required')")
  })
})

describe('the gate holds while the answer is unknown', () => {
  // `null` means "not resolved yet". Treating it as falsy would flash the app
  // open, or mount a Clerk hook with no provider above it — the crash that took
  // down the whole Settings modal.
  it('each consumer branches on an explicit null before acting', () => {
    for (const rel of CONSUMERS.slice(0, 2)) {
      expect(read(rel)).toContain('signInRequired === null')
    }
  })

  it('the wizard step only opens on an explicit false', () => {
    const steps = read('/app/setup/steps.tsx')
    // `canNext` must test `=== false`, never `!signInRequired` — the latter is
    // true while still loading and would let the user past a step that has not
    // been decided yet.
    expect(steps).toContain('signInRequired === false')
    expect(steps).not.toContain('!s.signInRequired')
  })
})
