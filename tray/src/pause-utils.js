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
 * (empty, non-numeric, zero, or negative).
 */
function parsePauseMins(raw) {
  const n = parseInt(raw, 10)
  if (!n || n < 1) return null
  return n
}

/**
 * Build a human-readable label for the pause-duration toast notification.
 * Mirrors the inline logic inside pause_for_duration in daemon.rs.
 *   0 s        → "N seconds"
 *   1–59 min   → "N minute(s)"
 *   ≥ 60 min   → "N hour(s)"
 */
function pauseLabel(seconds) {
  const mins = Math.floor(seconds / 60)
  if (mins === 0) return `${seconds} second${seconds === 1 ? '' : 's'}`
  if (mins >= 60) {
    const h = Math.floor(mins / 60)
    return `${h} hour${h === 1 ? '' : 's'}`
  }
  return `${mins} minute${mins === 1 ? '' : 's'}`
}

// Dual-mode export: globals when loaded via <script>, module when require()'d
// by the vitest suite.
if (typeof module !== 'undefined') {
  module.exports = { fmtCountdown, parsePauseMins, pauseLabel }
}
