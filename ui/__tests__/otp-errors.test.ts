//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Coverage for `ui/lib/otp-errors.ts`, the sentinel-string → copy mapping
// `OtpForm.tsx` renders. Extracted as pure functions specifically so this is
// testable at all — this codebase's test suite (bun test) has no DOM/React
// rendering harness, so a component's behavior is only pinned through logic
// it delegates to a plain function (the same shape `signin-required.test.ts`
// used for `resolveSignInRequired` before this change removed both).
//
// Also source-scans `OtpForm.tsx` for a few static invariants a headless run
// cannot exercise: no leftover `@clerk/react`/`tauri-plugin-clerk` import, and
// the plain-hyphen rule (CLAUDE.md's Hard Rules) for user-facing copy.

import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'node:fs'
import { classifySendError, classifyVerifyError } from '../lib/otp-errors'
import { stripComments } from './helpers/source'

const uiRoot = import.meta.dir + '/..'

describe('classifySendError', () => {
  it('maps invalid_email to a user-fixable message', () => {
    expect(classifySendError('invalid_email').message).toContain('valid email')
    expect(classifySendError('invalid_email').isDevBypass).toBe(false)
  })

  it('maps rate_limited and unavailable to distinct retry copy', () => {
    const rateLimited = classifySendError('rate_limited').message
    const unavailable = classifySendError('unavailable').message
    expect(rateLimited).not.toEqual(unavailable)
    expect(rateLimited.toLowerCase()).toContain('too many')
    expect(unavailable.toLowerCase()).toContain('unavailable')
  })

  it('is the ONLY sentinel that sets isDevBypass — not_configured', () => {
    expect(classifySendError('not_configured').isDevBypass).toBe(true)
    for (const other of ['invalid_email', 'rate_limited', 'unavailable', 'unauthorized', 'garbage', '']) {
      expect(classifySendError(other).isDevBypass).toBe(false)
    }
  })

  it('unauthorized (an attestation/config failure) shows the generic message, never a config hint', () => {
    // An unauthorized bearer token is a build problem, not something the user
    // can fix — it must not be confused with `not_configured`'s dev-notice copy.
    const msg = classifySendError('unauthorized').message
    expect(msg.toLowerCase()).not.toContain('otp_api_url')
  })

  it('falls back to the generic message for an unrecognised sentinel or a real Error', () => {
    expect(classifySendError('unexpected_status:500').message).toContain('Something went wrong')
    expect(classifySendError(new Error('network down')).message).toContain('Something went wrong')
    expect(classifySendError(undefined).message).toContain('Something went wrong')
  })
})

describe('classifyVerifyError', () => {
  // The Worker deliberately returns the same signal ("expired") for BOTH "no
  // live code for this email" and "you used up your 5 attempts" - see
  // infra/otp-worker/src/otp.ts's VerifyOutcome doc - so there is no separate
  // "too_many_attempts" sentinel to classify here anymore.
  it('maps expired to a request-a-new-code message', () => {
    expect(classifyVerifyError('expired').message.toLowerCase()).toContain('request a new')
  })

  it('flags not_configured as the dev-bypass case', () => {
    expect(classifyVerifyError('not_configured').isDevBypass).toBe(true)
    expect(classifyVerifyError('expired').isDevBypass).toBe(false)
  })

  it('falls back to the generic message for anything unrecognised', () => {
    expect(classifyVerifyError('unexpected_status:500').message).toContain('Something went wrong')
  })
})

describe('user-facing OTP copy follows the plain-hyphen rule', () => {
  it('never uses an em-dash, en-dash, or double hyphen', () => {
    const sentinels = [
      'invalid_email', 'rate_limited', 'unavailable', 'unauthorized', 'not_configured', 'garbage',
    ]
    const messages = [
      ...sentinels.map((s) => classifySendError(s).message),
      ...sentinels.map((s) => classifyVerifyError(s).message),
      ...['expired', 'invalid_input'].map((s) => classifyVerifyError(s).message),
      ...['blocked'].map((s) => classifySendError(s).message),
    ]
    for (const m of messages) expect(m).not.toMatch(/[—–]|--/)
  })
})

describe('OtpForm.tsx', () => {
  // Comments stripped first (CLAUDE.md's memory: `include_str!ing`/scanning
  // your own historical-context prose is how a source scan matches itself —
  // this file's own module doc mentions "@clerk/react" by name, in prose
  // explaining what it replaced).
  const src = stripComments(readFileSync(`${uiRoot}/app/setup/signin/OtpForm.tsx`, 'utf8'))

  it('has no leftover Clerk dependency', () => {
    expect(src).not.toContain('@clerk/react')
    expect(src).not.toContain('tauri-plugin-clerk')
    expect(src).not.toContain('useSignIn')
    expect(src).not.toContain('useSignUp')
  })

  it('calls the OTP commands, not a Clerk API', () => {
    expect(src).toContain("'request_account_otp'")
    expect(src).toContain("'confirm_account_otp'")
  })

  it('saves the email itself before reporting a successful sign-in up', () => {
    // Per the plan: OtpForm calls save_account_email on a verified code, then
    // the passed-in onSignedIn — confirm_account_otp itself never does this.
    const saveIdx = src.indexOf("'save_account_email'")
    const onSignedInCallIdx = src.indexOf('onSignedIn(email)')
    expect(saveIdx).toBeGreaterThan(-1)
    expect(onSignedInCallIdx).toBeGreaterThan(saveIdx)
  })
})
