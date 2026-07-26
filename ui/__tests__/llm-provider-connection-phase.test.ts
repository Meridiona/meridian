//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The three states a provider card can be in, and what the user sees for each:
//
//   1. NOT INSTALLED AT ALL       — probed.installed === false
//   2. INSTALLED, NOT AUTHENTICATED — probed.installed === true, but the real "Test" call
//      (test_llm_provider -> src/llm/detect.rs::test_provider, a genuine live request to the
//      CLI — this IS the "send a live request and check the response" button) came back
//      failed. Meridian never claims to know auth state up front (`authenticated` is always
//      null — see api-types.ts) — "signed in or not" is only ever answered by actually trying
//      a real call, which is exactly what the Test button does.
//   3. INSTALLED, AUTHENTICATED, WORKING — the same real call came back `{status: 'ok'}`.
//
// `phaseFor` is imported and exercised for real — it is the single place these three states
// (plus the transient installing/signing/testing ones) collapse into what the card renders.
// The rest is scanned from source — this repo has no React render harness (see
// worklog-propose-provider.test.ts).

import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { phaseFor } from '../components/LlmProviderDetail'
import type { ProviderStatus, ProviderTestResult } from '../lib/api-types'

const src = (p: string) => readFileSync(join(import.meta.dir, '..', p), 'utf8')
const detail = src('components/LlmProviderDetail.tsx')

const status = (installed: boolean, path: string | null = null): ProviderStatus => ({
  id: 'codex',
  installed,
  path,
  authenticated: null,
  last_test: null,
})

const testResult = (outcome: ProviderTestResult['outcome']): ProviderTestResult => ({
  id: 'codex',
  outcome,
  elapsed_ms: 1234,
  tested_at: '2026-07-24T12:00:00Z',
})

describe('phaseFor — scenario 1: not installed at all', () => {
  it('is not_installed once probed and absent, with no prior test on record', () => {
    expect(phaseFor(false, false, false, status(false), null).kind).toBe('not_installed')
  })

  it('is STILL not_installed even if a stale last_test says ok — a since-uninstalled CLI must not read as working', () => {
    const stale = testResult({ status: 'ok' })
    expect(phaseFor(false, false, false, status(false), stale).kind).toBe('not_installed')
  })

  it('is unknown, not not_installed, when the probe itself has not resolved yet (no false "missing" flash on mount)', () => {
    expect(phaseFor(false, false, false, undefined, null).kind).toBe('unknown')
  })
})

describe('phaseFor — scenario 2: installed, not authenticated', () => {
  it('is failed once a real Test call comes back failed', () => {
    const failed = testResult({ status: 'failed', message: 'not authenticated' })
    const phase = phaseFor(false, false, false, status(true, '/usr/local/bin/codex'), failed)
    expect(phase.kind).toBe('failed')
    expect(phase.kind === 'failed' && phase.message).toBe('not authenticated')
  })

  it('is ready_untested when installed but no Test has been run yet — the state before the user has clicked anything', () => {
    const phase = phaseFor(false, false, false, status(true, '/usr/local/bin/codex'), null)
    expect(phase.kind).toBe('ready_untested')
  })

  it('is rate_limited, not failed, when the account is signed in but temporarily capped — a different fix than "sign in"', () => {
    const limited = testResult({ status: 'rate_limited', message: 'usage limit' })
    const phase = phaseFor(false, false, false, status(true), limited)
    expect(phase.kind).toBe('rate_limited')
  })
})

describe('phaseFor — scenario 3: installed, authenticated, working', () => {
  it('is ok once a real Test call succeeds', () => {
    const ok = testResult({ status: 'ok' })
    expect(phaseFor(false, false, false, status(true, '/usr/local/bin/codex'), ok).kind).toBe('ok')
  })
})

describe('phaseFor — transient / in-flight states win over any stale result', () => {
  it('installing beats everything else', () => {
    const ok = testResult({ status: 'ok' })
    expect(phaseFor(true, false, false, status(true), ok).kind).toBe('installing')
  })

  it('signing beats everything else', () => {
    expect(phaseFor(false, true, false, status(true), null).kind).toBe('signing')
  })

  it('testing (the Test button, in flight) beats a stale prior result', () => {
    const failed = testResult({ status: 'failed', message: 'old failure' })
    expect(phaseFor(false, false, true, status(true), failed).kind).toBe('testing')
  })
})

// ── The Test button really does send one real request — not a canned/local check ──────────

it('onTest is wired straight to the real backend test command, not a local heuristic', () => {
  // detail.tsx takes onTest as a prop and never computes install/auth state itself — the
  // picker (one level up) owns the actual invoke() call. Assert the prop is threaded through
  // to both places a user can trigger it, so a future refactor can't quietly stub it out.
  expect(detail).toMatch(/onTest\s*:\s*\(\)\s*=>\s*void/)
  expect(detail).toMatch(/onClick=\{onTest\}/)
})

const picker = src('components/LlmProviderPicker.tsx')

it("the picker's test handler invokes the real Tauri command against the live CLI, not a mock", () => {
  expect(picker).toMatch(/invoke<ProviderTestResult>\('test_llm_provider'/)
})

// ── Subscription providers (Cursor, Codex) get bespoke "not signed in" copy with an in-app ──
// ── sign-in button for an unclassified failure; every other provider shows the raw message. ──
// ── Both must still originate from the SAME real Test call (a `failed` phase).

it('the subscription-provider "not signed in" copy only replaces the DISPLAY, not the trigger — it is still gated on phase.kind === "failed"', () => {
  const failedBranch = detail.slice(detail.indexOf("case 'failed':"))
  // The in-app sign-in UI is gated on the failed phase via the signInProvider descriptor,
  // showing the "not signed in" copy and that provider's own sign-in button label.
  expect(failedBranch).toMatch(/if \(signInProvider\)/)
  expect(failedBranch).toMatch(/not signed in yet/)
  expect(failedBranch).toMatch(/\{signInProvider\.label\}/)
  // Both Cursor and Codex are covered by the descriptor (each with its own bespoke label).
  expect(detail).toMatch(/Sign in to Cursor/)
  expect(detail).toMatch(/Sign in to Codex/)
})

it('every other provider surfaces the real failure message verbatim, not a generic substitute', () => {
  expect(detail).toMatch(/isn&apos;t responding: \{phase\.message\}/)
})
