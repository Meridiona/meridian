//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, it, expect, afterEach } from 'bun:test'
import { isLikelyClerkNetworkError } from '../lib/clerkNetworkError'

// Regression coverage for a real report: a staging build showed "Sign-in
// couldn't start - check CLERK_PUBLISHABLE_KEY and restart" on a Mac right
// after a machine restart, with a correctly-configured key. Root cause: the
// tray auto-launches at login before the OS has network up, so `clerk.load()`
// rejects on a connection failure - `ClerkErrorBoundary` showed the
// misconfiguration message for what was actually a transient offline window.
// This classifier is what lets the boundary tell the two apart.

describe('isLikelyClerkNetworkError', () => {
  afterEach(() => {
    // @ts-expect-error -- test-only override of a read-only DOM property
    delete global.navigator
  })

  it('is true for a reqwest/clerk-fapi-rs connection failure (Rust side)', () => {
    expect(isLikelyClerkNetworkError(new Error(
      'error trying to connect: dns error: failed to lookup address information: nodename nor servname provided, or not known',
    ))).toBe(true)
  })

  it('is true for a plain "error sending request" reqwest wrapper', () => {
    expect(isLikelyClerkNetworkError(new Error('error sending request for url (https://clerk.meridiona.com/v1/client)'))).toBe(true)
  })

  it('is true for the browser fetch failure wording (JS-side Clerk load)', () => {
    expect(isLikelyClerkNetworkError(new Error('Failed to fetch'))).toBe(true)
    expect(isLikelyClerkNetworkError(new Error('NetworkError when attempting to fetch resource.'))).toBe(true)
  })

  it('is true for a Safari-style offline message', () => {
    expect(isLikelyClerkNetworkError('The Internet connection appears to be offline.')).toBe(true)
  })

  it('is true for a Chromium net-error code', () => {
    expect(isLikelyClerkNetworkError(new Error('net::ERR_INTERNET_DISCONNECTED'))).toBe(true)
  })

  it('is true whenever the browser itself reports offline, regardless of message', () => {
    // @ts-expect-error -- test-only global override
    global.navigator = { onLine: false }
    expect(isLikelyClerkNetworkError(new Error('some completely unrelated error text'))).toBe(true)
  })

  it('is false for a malformed/wrong-instance publishable key error', () => {
    expect(isLikelyClerkNetworkError(new Error('Missing publishableKey'))).toBe(false)
    expect(isLikelyClerkNetworkError(new Error('Clerk: Missing publishable_key'))).toBe(false)
  })

  it('is false for an unrelated JS error', () => {
    expect(isLikelyClerkNetworkError(new TypeError('Cannot read properties of undefined'))).toBe(false)
  })

  it('handles non-Error rejection shapes without throwing', () => {
    expect(isLikelyClerkNetworkError(undefined)).toBe(false)
    expect(isLikelyClerkNetworkError(null)).toBe(false)
    expect(isLikelyClerkNetworkError({ code: 'native_api_disabled' })).toBe(false)
  })
})
