//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

// Classifies why `tauri-plugin-clerk`'s `initClerk()` rejected: a machine with
// no network yet (the login-item auto-launch races the OS's own network
// bring-up, so `clerk.load()`'s live-API fetch has nothing to reach) versus an
// actual misconfiguration (a malformed/wrong-instance publishable key).
// `ClerkErrorBoundary` uses this to pick copy that matches the real cause -
// "check CLERK_PUBLISHABLE_KEY" is true for the second case and actively wrong
// for the first, since the key is fine and the fix is just waiting for a
// connection.
//
// The Rust side (`clerk.load()` via clerk-fapi-rs -> reqwest) and the JS side
// (`@clerk/clerk-js`'s own fetch, and the CDN fetch for prebuilt UI) can each
// be the one that rejects, on either OS, so this matches substrings from both
// stacks rather than one platform's wording. False negatives (a network error
// misread as "other") just fall back to the pre-existing generic message, so
// the list is kept broad on purpose.
const NETWORK_ERROR_PATTERNS = [
  'error trying to connect',
  'error sending request',
  'dns error',
  'failed to lookup address',
  'connection refused',
  'network is unreachable',
  'network is down',
  'could not connect',
  'timed out',
  'operation timed out',
  'failed to fetch',
  'load failed',
  'networkerror',
  'internet connection appears to be offline',
  'err_internet_disconnected',
  'err_network_changed',
  'err_name_not_resolved',
]

function errorText(error: unknown): string {
  if (error instanceof Error) return error.message
  if (typeof error === 'string') return error
  if (error === null || error === undefined) return ''
  try {
    // JSON.stringify returns `undefined` (not a string) for values like a bare
    // function, which would otherwise crash the caller's .toLowerCase().
    return JSON.stringify(error) ?? String(error)
  } catch {
    return String(error)
  }
}

/** True when `initClerk()`'s rejection looks like "no network yet", not a bad
 *  key. Combines two independent signals: the browser's own connectivity flag
 *  (unavailable outside a browser-like runtime, hence the `typeof` guard) and
 *  substring-matching the error text against known network-failure wording
 *  from reqwest, `@clerk/clerk-js`'s fetch, and Chromium's net-error codes. */
export function isLikelyClerkNetworkError(error: unknown): boolean {
  const offline = typeof navigator !== 'undefined' && navigator.onLine === false
  if (offline) return true
  const text = errorText(error).toLowerCase()
  return NETWORK_ERROR_PATTERNS.some((pattern) => text.includes(pattern))
}
