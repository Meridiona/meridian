//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Server-side helpers for the notification outbox (table `notifications`, written
// by the Rust daemon via src/notifications.rs). This is the single delivery layer:
// preference + quiet-hours filtering lives here, so producers always enqueue and
// only the user's settings decide whether an event surfaces.

import type Database from 'better-sqlite3'
import { readSettings, type RuntimeSettings } from '@/lib/settings'

export interface NotificationRow {
  id: number
  dedup_key: string
  event_key: string
  severity: 'info' | 'warning' | 'error'
  title: string
  body: string
  deep_link: string | null
  channels: string
  scheduled_for: string | null
  expires_at: string | null
  delivered_native_at: string | null
  banner_dismissed_at: string | null
  attempts: number
  created_at: string
}

// Map an event_key to its per-type preference. Unknown event keys default to
// enabled (a new producer is visible until the user opts out), gated only by the
// master switch.
function typeEnabled(eventKey: string, s: RuntimeSettings): boolean {
  switch (eventKey) {
    case 'plan.nudge':    return s.notify_plan_nudge
    case 'worklog.ready': return s.notify_worklog_ready
    case 'system.fault':  return s.notify_system_fault
    default:              return true
  }
}

/** Whether `event_key` may surface at all, per the master switch + per-type toggle. */
export function eventAllowed(eventKey: string, s: RuntimeSettings = readSettings()): boolean {
  return s.notifications_enabled && typeEnabled(eventKey, s)
}

/** Minutes since local midnight for an 'HH:MM' string, or null if malformed. */
function hhmmToMinutes(hhmm: string): number | null {
  const m = /^(\d{1,2}):(\d{2})$/.exec(hhmm.trim())
  if (!m) return null
  const h = Number(m[1]); const min = Number(m[2])
  if (h > 23 || min > 59) return null
  return h * 60 + min
}

/**
 * True if `now` (local) falls inside the configured quiet-hours window. Handles
 * windows that wrap past midnight (e.g. 22:00→08:00). Returns false when quiet
 * hours are disabled or the bounds are malformed (fail open — better to notify
 * than to silently swallow).
 */
export function inQuietHours(s: RuntimeSettings = readSettings(), now: Date = new Date()): boolean {
  if (!s.quiet_hours_enabled) return false
  const start = hhmmToMinutes(s.quiet_hours_start)
  const end = hhmmToMinutes(s.quiet_hours_end)
  if (start === null || end === null || start === end) return false
  const cur = now.getHours() * 60 + now.getMinutes()
  return start < end
    ? cur >= start && cur < end          // same-day window
    : cur >= start || cur < end          // wraps past midnight
}

const SELECT_COLS =
  'id, dedup_key, event_key, severity, title, body, deep_link, channels, ' +
  'scheduled_for, expires_at, delivered_native_at, banner_dismissed_at, attempts, created_at'

const NOW_ISO = () => new Date().toISOString().replace(/\.\d+Z$/, 'Z')

/** Does a CSV channel set include `channel`? */
function hasChannel(channels: string, channel: string): boolean {
  return channels.split(',').map(c => c.trim()).includes(channel)
}

/**
 * Native-channel rows ready to fire: undelivered, due (scheduled_for past),
 * unexpired, channel includes 'native', and allowed by prefs + quiet hours.
 * FIFO by id. The tray polls this.
 */
export function pendingNative(db: Database.Database): NotificationRow[] {
  const now = NOW_ISO()
  const rows = db.prepare(
    `SELECT ${SELECT_COLS} FROM notifications
      WHERE delivered_native_at IS NULL
        AND (scheduled_for IS NULL OR scheduled_for <= ?)
        AND (expires_at IS NULL OR expires_at > ?)
      ORDER BY id ASC`,
  ).all(now, now) as NotificationRow[]
  const settings = readSettings()
  const quiet = inQuietHours(settings)
  return rows.filter(r =>
    hasChannel(r.channels, 'native') &&
    eventAllowed(r.event_key, settings) &&
    !quiet,
  )
}

/**
 * Banner-channel rows to display: undismissed, unexpired, channel includes
 * 'banner', allowed by prefs (quiet hours do NOT gate banners — they're passive,
 * not interruptive). Newest first.
 */
export function activeBanners(db: Database.Database): NotificationRow[] {
  const now = NOW_ISO()
  const rows = db.prepare(
    `SELECT ${SELECT_COLS} FROM notifications
      WHERE banner_dismissed_at IS NULL
        AND (expires_at IS NULL OR expires_at > ?)
      ORDER BY id DESC`,
  ).all(now) as NotificationRow[]
  const settings = readSettings()
  return rows.filter(r => hasChannel(r.channels, 'banner') && eventAllowed(r.event_key, settings))
}

/** Mark a row delivered on the native channel (idempotent). */
export function markNativeDelivered(db: Database.Database, id: number): void {
  db.prepare(
    'UPDATE notifications SET delivered_native_at = ?, attempts = attempts + 1 WHERE id = ? AND delivered_native_at IS NULL',
  ).run(NOW_ISO(), id)
}

/** Mark a row's banner dismissed (idempotent). */
export function dismissBanner(db: Database.Database, id: number): void {
  db.prepare(
    'UPDATE notifications SET banner_dismissed_at = ? WHERE id = ? AND banner_dismissed_at IS NULL',
  ).run(NOW_ISO(), id)
}
