//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Replacement coverage for what `signin-required.test.ts` used to pin before
// this change deleted both it and `RequireSignIn.tsx`.
//
// THE POLARITY FLIPPED ON PURPOSE. The old `resolveSignInRequired` failed
// CLOSED on a rejected `invoke` (a broken bridge must land ON the gate, since
// `RequireSignIn` was hiding a real, currently-enforced live session
// requirement — defaulting open would have let anyone past a sign-in that was
// actually still required). `RequireEmailCapture` is a ONE-TIME capture, not a
// session: there is nothing to bypass once an email has ever been saved, so a
// transient IPC hiccup must never re-lock an already-captured user. The cost
// of failing open here is "show the capture form again, harmlessly" — never a
// security hole, unlike the case the old test guarded.

import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'node:fs'
import { resolveCaptureState } from '../app/setup/signin/RequireEmailCapture'
import { stripComments } from './helpers/source'

const uiRoot = import.meta.dir + '/..'
const read = (rel: string) => stripComments(readFileSync(uiRoot + rel, 'utf8'))

describe('resolveCaptureState', () => {
  it('is needs_capture when nothing has ever been saved', async () => {
    expect(await resolveCaptureState(async () => null)).toBe('needs_capture')
  })

  it('is ready when an email is already captured', async () => {
    expect(await resolveCaptureState(async () => 'user@example.com')).toBe('ready')
  })

  it('FAILS OPEN when the command rejects — the inverted case', async () => {
    const boom = async () => { throw new Error('command not found') }
    expect(await resolveCaptureState(boom)).toBe('ready')
  })

  it('fails open when the command rejects with a non-Error (Tauri rejects with a plain string)', async () => {
    const boom = () => Promise.reject('unknown command: get_account_email')
    expect(await resolveCaptureState(boom)).toBe('ready')
  })

  it('always settles, so a caller can never hang on it', async () => {
    const hung = Promise.race([
      resolveCaptureState(() => Promise.reject(new Error('x'))),
      new Promise((_, rej) => setTimeout(() => rej(new Error('never settled')), 1000)),
    ])
    expect(await hung).toBe('ready')
  })
})

describe('RequireEmailCapture.tsx', () => {
  const src = read('/app/setup/signin/RequireEmailCapture.tsx')

  it('has no leftover Clerk dependency', () => {
    expect(src).not.toContain('@clerk/react')
    expect(src).not.toContain('ClerkGate')
    expect(src).not.toContain('useSignInRequired')
  })

  it('checks the Tauri runtime before ever calling invoke', () => {
    // Mirrors the deleted ClerkGate's non-Tauri fallback: outside the webview
    // there is no bridge to reach at all.
    expect(src).toContain('isTauri()')
  })

  it('renders through the shared resolver rather than re-rolling the invoke call inline', () => {
    expect(src).toContain('resolveCaptureState()')
  })
})
