//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use strict'

// Pure pause-feature helpers — no DOM access, no __TAURI__ references.
// Loaded via <script> in the browser (functions become globals) and imported
// via module.exports in the vitest test suite.

/** Format a remaining-ms countdown as "M:SS" (e.g. 330000 → "5:30"). */
function fmtCountdown(remainMs) {
  const total = Math.max(0, Math.ceil(remainMs / 1000))
  const m = Math.floor(total / 60)
  const s = total % 60
  return `${m}:${String(s).padStart(2, '0')}`
}

/**
 * Parse and validate a raw string value from the custom-duration input.
 * Returns the integer minutes on success, or null when the value is invalid
 * (empty, non-numeric, zero, negative, or above the 8-hour cap).
 * The HTML input carries max="480" but that attribute is bypassable via
 * DevTools / programmatic assignment, so validation must live here too.
 */
function parsePauseMins(raw) {
  const n = parseInt(raw, 10)
  if (!n || n < 1 || n > 480) return null
  return n
}

// Dual-mode export: fmtCountdown and parsePauseMins become browser globals
// (used by app.js); pauseLabel is exported for test parity with daemon.rs's
// pause_label() but is NOT a browser global — the actual toast is generated
// in Rust and the global name would serve no purpose in app.js.
if (typeof module !== 'undefined') {
  // pauseLabel mirrors Rust's pause_label() (daemon.rs) for test coverage only.
  //   0 s      → "N second(s)"   1–59 min → "N minute(s)"   ≥60 min → "N hour(s)"
  const pauseLabel = (seconds) => {
    const mins = Math.floor(seconds / 60)
    if (mins === 0) return `${seconds} second${seconds === 1 ? '' : 's'}`
    if (mins >= 60) {
      const h = Math.floor(mins / 60)
      return `${h} hour${h === 1 ? '' : 's'}`
    }
    return `${mins} minute${mins === 1 ? '' : 's'}`
  }
  module.exports = { fmtCountdown, parsePauseMins, pauseLabel }
}
